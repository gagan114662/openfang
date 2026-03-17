#!/usr/bin/env python3
"""Deterministic PR preflight gate for OpenFang harness engineering."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
from typing import Any, Dict, List, Tuple

from browser_evidence_verify import verify_manifest
from checks_resolver import (
    compute_required_checks,
    compute_risk_tier,
    evaluate_docs_drift,
    get_rollout_settings,
    load_contract,
    read_changed_files,
    requires_browser_evidence,
)
from greptile_state import (
    ReviewState,
    count_actionable_findings,
    get_review_check_state_once,
    load_or_init_review_findings,
    review_state_as_dict,
    wait_for_review_check,
    write_review_findings,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="OpenFang risk policy gate")
    parser.add_argument("--pr", type=int, required=True, help="Pull request number")
    parser.add_argument("--head-sha", required=True, help="Current PR head SHA")
    parser.add_argument("--changed-files", required=True, help="Path to newline-delimited changed files list")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Path to machine-readable harness contract")
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="owner/repo")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Environment variable that stores GitHub token")
    parser.add_argument("--review-findings", default="artifacts/review-findings.json", help="Input/output JSON with normalized primary review findings")
    parser.add_argument("--claude-findings", default="artifacts/claude-findings.json", help="Input JSON with normalized Claude findings")
    parser.add_argument("--browser-evidence-manifest", default="artifacts/browser-evidence-manifest.json", help="Browser evidence manifest path")
    parser.add_argument("--infra-preflight-report", default="artifacts/infra-preflight-report.json", help="Infra preflight JSON report path")
    parser.add_argument("--live-provider-report", default="artifacts/live-provider-report.json", help="Live provider gate JSON report path")
    parser.add_argument("--report-out", default="artifacts/risk-policy-report.json", help="Output path for risk-policy-report.json")
    parser.add_argument("--poll-seconds", type=int, default=20, help="Review check polling interval")
    return parser.parse_args()


def _default_review_state(provider: str, reason: str) -> ReviewState:
    return ReviewState(provider=provider, status="missing", details=reason)


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _read_optional_json(path: str) -> Dict[str, Any]:
    target = Path(path)
    if not target.exists():
        return {}
    try:
        payload = json.loads(target.read_text(encoding="utf-8"))
        if isinstance(payload, dict):
            return payload
    except Exception:
        return {}
    return {}


def _provider_config(contract: Dict[str, Any], provider: str) -> Dict[str, Any]:
    providers = contract.get("reviewProviders", {}).get("providers", {})
    payload = providers.get(provider, {})
    return payload if isinstance(payload, dict) else {}


def _provider_enforcement(contract: Dict[str, Any], provider: str, default: str = "required") -> str:
    cfg = _provider_config(contract, provider)
    value = str(cfg.get("enforcement", default)).strip().lower()
    return value if value in {"required", "advisory", "disabled"} else default


def _count_actionable(payload: Dict[str, Any]) -> int:
    findings = payload.get("findings", [])
    if not isinstance(findings, list):
        return 0
    return sum(1 for finding in findings if isinstance(finding, dict) and bool(finding.get("actionable")))


def _load_claude_payload(path: str, head_sha: str) -> Dict[str, Any]:
    payload = _read_optional_json(path)
    if not payload:
        return {
            "provider": "claude",
            "status": "missing",
            "head_sha": head_sha,
            "findings": [],
            "errors": ["claude findings payload missing"],
        }

    payload.setdefault("provider", "claude")
    payload.setdefault("status", "missing")
    payload.setdefault("head_sha", head_sha)
    payload.setdefault("findings", [])
    payload.setdefault("errors", [])

    if str(payload.get("head_sha", "")) != head_sha:
        payload["status"] = "missing"
        errors = payload.get("errors", [])
        if not isinstance(errors, list):
            errors = []
        errors.append("claude findings are stale for current head SHA")
        payload["errors"] = errors
    return payload


def _infra_or_live_failed(report: Dict[str, Any]) -> Tuple[bool, str]:
    if not report:
        return False, ""

    status = str(report.get("status", "")).lower()
    if status in {"fail", "failure", "error", "failed"}:
        return True, status

    for key in ("ok", "pass", "passed", "success"):
        if key in report:
            try:
                if bool(report.get(key)):
                    return False, ""
                return True, f"{key}=false"
            except Exception:
                return True, f"{key}=invalid"

    return False, ""


def main() -> int:
    args = parse_args()

    contract = load_contract(args.contract)
    changed_files = read_changed_files(args.changed_files)

    risk_tier_rules = contract.get("riskTierRules", {})
    if not risk_tier_rules:
        legacy = contract.get("riskTiers", {})
        risk_tier_rules = {
            str(name): [str(path) for path in tier_payload.get("paths", [])]
            for name, tier_payload in legacy.items()
            if isinstance(tier_payload, dict)
        }

    risk_tier = compute_risk_tier(changed_files, risk_tier_rules)
    required_checks = compute_required_checks(contract, risk_tier)
    rollout_phase, rollout_settings = get_rollout_settings(contract)

    review_policy = contract.get("reviewPolicy", {})
    primary_provider = str(review_policy.get("provider", "greptile"))
    check_name = str(review_policy.get("checkRunName", "greptile-review"))
    timeout_minutes = int(review_policy.get("timeoutMinutes", 20))
    weak_confidence = float(review_policy.get("weakConfidenceThreshold", 0.55))
    keywords = [str(k).lower() for k in review_policy.get("actionableSummaryKeywords", [])]

    primary_enforcement = _provider_enforcement(contract, primary_provider, default="required")
    enforce_review_state = bool(rollout_settings.get("enforceReviewState", False) or primary_enforcement != "advisory")

    token = os.getenv(args.token_env, "")
    review_state: ReviewState
    if args.repo and token:
        if enforce_review_state:
            review_state = wait_for_review_check(
                repo=args.repo,
                sha=args.head_sha,
                token=token,
                check_name=check_name,
                timeout_minutes=timeout_minutes,
                poll_seconds=args.poll_seconds,
                provider=primary_provider,
            )
        else:
            review_state = get_review_check_state_once(
                repo=args.repo,
                sha=args.head_sha,
                token=token,
                check_name=check_name,
                provider=primary_provider,
            )
    else:
        missing_reason = "review API unavailable (missing repo or token); cannot verify current-head review state"
        review_state = _default_review_state(primary_provider, missing_reason)

    review_findings = load_or_init_review_findings(
        args.review_findings,
        head_sha=args.head_sha,
        provider=primary_provider,
        weak_confidence_threshold=weak_confidence,
        actionable_keywords=keywords,
    )
    review_findings["status"] = review_state.status
    write_review_findings(args.review_findings, review_findings)
    actionable_findings_count = count_actionable_findings(review_findings)

    claude_cfg = _provider_config(contract, "claude")
    claude_enforcement = _provider_enforcement(contract, "claude", default="required")
    claude_require_ingestion = bool(claude_cfg.get("requireCurrentHeadIngestion", True))
    claude_findings = _load_claude_payload(args.claude_findings, args.head_sha)
    claude_status = str(claude_findings.get("status", "missing")).lower()
    claude_actionable = _count_actionable(claude_findings)

    docs_violations = evaluate_docs_drift(changed_files, contract.get("docsDriftRules", []))

    evidence_policy = contract.get("evidencePolicy", {})
    evidence_needed = requires_browser_evidence(changed_files, contract)
    evidence_errors: List[str] = []
    if evidence_needed:
        evidence_ok, evidence_errors, _ = verify_manifest(
            args.browser_evidence_manifest,
            head_sha=args.head_sha,
            required_flows=evidence_policy.get("requiredFlows", []),
            required_assertions=evidence_policy.get("requiredAssertions", []),
        )
        if evidence_ok:
            evidence_errors = []

    infra_preflight_report = _read_optional_json(args.infra_preflight_report)
    live_provider_report = _read_optional_json(args.live_provider_report)

    decision = "pass"
    reasons: List[str] = []

    enforce_docs_drift = bool(rollout_settings.get("enforceDocsDrift", False))
    enforce_evidence = bool(rollout_settings.get("requireEvidence", False))
    enable_remediation = bool(rollout_settings.get("enableRemediation", False))

    def record_reason(message: str, *, enforced: bool) -> None:
        if enforced:
            reasons.append(message)
        else:
            reasons.append(f"advisory: {message}")

    if review_state.status == "timeout":
        record_reason("review check timed out on current head SHA", enforced=enforce_review_state)
        if enforce_review_state:
            decision = "timeout"
    elif review_state.status == "pending":
        record_reason("current-head review is still pending", enforced=enforce_review_state)
        if enforce_review_state:
            decision = "stale-review"
    elif review_state.status in {"missing"}:
        record_reason("current-head review is missing", enforced=enforce_review_state)
        if enforce_review_state:
            decision = "stale-review"
    elif review_state.status in {"failure", "error"}:
        record_reason("review check is not successful on current head SHA", enforced=enforce_review_state)
        if enforce_review_state:
            decision = "fail"

    if actionable_findings_count > 0:
        if enable_remediation:
            record_reason(f"{actionable_findings_count} actionable {primary_provider} finding(s) detected", enforced=True)
            if decision == "pass":
                decision = "needs-remediation"
        else:
            enforced = primary_enforcement == "required"
            record_reason(
                f"{actionable_findings_count} actionable {primary_provider} finding(s) detected",
                enforced=enforced,
            )
            if enforced and decision == "pass":
                decision = "fail"

    if claude_enforcement != "disabled":
        claude_required = claude_enforcement == "required"
        if claude_status != "success":
            enforced = claude_required and claude_require_ingestion
            record_reason(
                f"claude findings ingestion status is '{claude_status}' for current head SHA",
                enforced=enforced,
            )
            if enforced and decision == "pass":
                decision = "fail"

        if claude_actionable > 0:
            if enable_remediation:
                record_reason(f"{claude_actionable} actionable claude finding(s) detected", enforced=True)
                if decision == "pass":
                    decision = "needs-remediation"
            else:
                record_reason(
                    f"{claude_actionable} actionable claude finding(s) detected",
                    enforced=claude_required,
                )
                if claude_required and decision == "pass":
                    decision = "fail"

    if docs_violations:
        for violation in docs_violations:
            record_reason(violation, enforced=enforce_docs_drift)
        if enforce_docs_drift and decision == "pass":
            decision = "fail"

    if evidence_needed and evidence_errors:
        for error in evidence_errors:
            record_reason(f"browser evidence: {error}", enforced=enforce_evidence)
        if enforce_evidence and decision == "pass":
            decision = "fail"

    infra_failed, infra_reason = _infra_or_live_failed(infra_preflight_report)
    if infra_failed:
        record_reason(f"infra preflight report failed ({infra_reason})", enforced=True)
        if decision == "pass":
            decision = "fail"

    live_failed, live_reason = _infra_or_live_failed(live_provider_report)
    if live_failed:
        record_reason(f"live provider report failed ({live_reason})", enforced=True)
        if decision == "pass":
            decision = "fail"

    if not reasons:
        reasons.append("all policy checks passed")

    report = {
        "pr_number": args.pr,
        "head_sha": args.head_sha,
        "risk_tier": risk_tier,
        "changed_files": changed_files,
        "required_checks": required_checks,
        "review_state": review_state_as_dict(review_state),
        "provider_states": {
            primary_provider: review_state_as_dict(review_state),
            "claude": {
                "provider": "claude",
                "status": claude_status,
                "details": "; ".join(str(item) for item in claude_findings.get("errors", []) if item),
            },
        },
        "actionable_findings_count": actionable_findings_count,
        "provider_actionable_findings": {
            primary_provider: actionable_findings_count,
            "claude": claude_actionable,
        },
        "decision": decision,
        "reasons": reasons,
        "timestamp": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "rollout_phase": rollout_phase,
        "rollout": rollout_settings,
        "infra_preflight": infra_preflight_report,
        "live_provider": live_provider_report,
    }

    _write_json(args.report_out, report)

    enforce_merge_block = bool(rollout_settings.get("enforceMergeBlock", False))
    should_fail_job = enforce_merge_block and decision != "pass"

    print(json.dumps(report, indent=2, sort_keys=True))
    return 1 if should_fail_job else 0


if __name__ == "__main__":
    raise SystemExit(main())
