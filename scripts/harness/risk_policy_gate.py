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
    get_evidence_policy,
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
    parser.add_argument(
        "--changed-files",
        required=True,
        help="Path to newline-delimited changed files list",
    )
    parser.add_argument(
        "--contract",
        default=".harness/policy.contract.json",
        help="Path to machine-readable harness contract",
    )
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="owner/repo")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Environment variable that stores GitHub token")
    parser.add_argument(
        "--review-findings",
        default="artifacts/review-findings.json",
        help="Input/output JSON with normalized review findings",
    )
    parser.add_argument(
        "--claude-findings",
        default="artifacts/claude-findings.json",
        help="Optional JSON with normalized Claude findings",
    )
    parser.add_argument(
        "--browser-evidence-manifest",
        default="artifacts/browser-evidence-manifest.json",
        help="Browser evidence manifest path",
    )
    parser.add_argument(
        "--infra-preflight-report",
        default="artifacts/infra-preflight-report.json",
        help="Infra preflight report path",
    )
    parser.add_argument(
        "--live-provider-report",
        default="artifacts/agent-evals/live-provider-report.json",
        help="Live provider probe report path",
    )
    parser.add_argument(
        "--report-out",
        default="artifacts/risk-policy-report.json",
        help="Output path for risk-policy-report.json",
    )
    parser.add_argument("--poll-seconds", type=int, default=20, help="Review check polling interval")
    return parser.parse_args()


def _default_review_state(provider: str, reason: str) -> ReviewState:
    return ReviewState(provider=provider, status="missing", details=reason)


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _read_json(path: str, default: Dict[str, Any]) -> Dict[str, Any]:
    file_path = Path(path)
    if not file_path.exists():
        return default
    try:
        payload = json.loads(file_path.read_text(encoding="utf-8"))
    except Exception:
        return default
    return payload if isinstance(payload, dict) else default


def _as_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        lowered = value.strip().lower()
        return lowered in {"1", "true", "yes", "on", "enabled", "labeled-only"}
    return False


def _as_int(value: Any, default: int) -> int:
    try:
        return int(value)
    except Exception:
        return default


def _as_float(value: Any, default: float) -> float:
    try:
        return float(value)
    except Exception:
        return default


def _normalize_provider_settings(contract: Dict[str, Any]) -> Tuple[str, str, Dict[str, Dict[str, Any]]]:
    legacy_review_policy = contract.get("reviewPolicy", {}) if isinstance(contract.get("reviewPolicy"), dict) else {}
    legacy_provider = str(legacy_review_policy.get("provider", "greptile") or "greptile")

    providers: Dict[str, Dict[str, Any]] = {
        legacy_provider: {
            "enabled": True,
            "enforcement": "required",
            "checkRunName": str(legacy_review_policy.get("checkRunName", "greptile-review")),
            "timeoutMinutes": _as_int(legacy_review_policy.get("timeoutMinutes"), 20),
            "weakConfidenceThreshold": _as_float(legacy_review_policy.get("weakConfidenceThreshold"), 0.55),
            "actionableSummaryKeywords": [
                str(k).lower() for k in legacy_review_policy.get("actionableSummaryKeywords", [])
            ],
        }
    }

    mode = "primary-only"
    primary = legacy_provider

    review_providers = contract.get("reviewProviders", {})
    if isinstance(review_providers, dict):
        mode = str(review_providers.get("mode", "alongside") or "alongside")
        primary = str(review_providers.get("primary", primary) or primary)

        candidate = review_providers.get("providers", {})
        if isinstance(candidate, dict) and candidate:
            providers = {}
            for name, cfg in candidate.items():
                if isinstance(cfg, dict):
                    providers[str(name)] = cfg

    if primary not in providers:
        providers[primary] = {
            "enabled": True,
            "enforcement": "required",
            "checkRunName": "greptile-review",
            "timeoutMinutes": 20,
            "weakConfidenceThreshold": 0.55,
            "actionableSummaryKeywords": [],
        }

    return mode, primary, providers


def _load_findings_artifact(path: str, *, head_sha: str, provider: str) -> Dict[str, Any]:
    artifact_path = Path(path)
    if not artifact_path.exists():
        return {
            "head_sha": head_sha,
            "provider": provider,
            "status": "missing",
            "findings": [],
        }

    try:
        payload = json.loads(artifact_path.read_text(encoding="utf-8"))
    except Exception:
        return {
            "head_sha": head_sha,
            "provider": provider,
            "status": "error",
            "findings": [],
        }

    if not isinstance(payload, dict):
        return {
            "head_sha": head_sha,
            "provider": provider,
            "status": "error",
            "findings": [],
        }

    payload.setdefault("head_sha", head_sha)
    payload.setdefault("provider", provider)
    payload.setdefault("status", "missing")
    payload.setdefault("findings", [])
    if not isinstance(payload.get("findings"), list):
        payload["findings"] = []
    return payload


def _review_state_from_findings(provider: str, findings_payload: Dict[str, Any]) -> ReviewState:
    status = str(findings_payload.get("status", "missing") or "missing")
    findings_count = len(findings_payload.get("findings", [])) if isinstance(findings_payload.get("findings"), list) else 0

    details = f"{provider} findings status={status}, findings={findings_count}"
    errors = findings_payload.get("errors", [])
    if isinstance(errors, list) and errors:
        details += f", errors={len(errors)}"

    return ReviewState(provider=provider, status=status, details=details)


def main() -> int:
    args = parse_args()

    contract = load_contract(args.contract)
    changed_files = read_changed_files(args.changed_files)
    risk_tier = compute_risk_tier(changed_files, contract)
    required_checks = compute_required_checks(contract, risk_tier)
    rollout_phase, rollout_settings = get_rollout_settings(contract)
    evidence_policy = get_evidence_policy(contract)

    mode, primary_provider, provider_cfg_map = _normalize_provider_settings(contract)
    primary_cfg = provider_cfg_map.get(primary_provider, {})
    primary_enforcement = str(primary_cfg.get("enforcement", "required") or "required").lower()

    check_name = str(primary_cfg.get("checkRunName", "greptile-review"))
    timeout_minutes = _as_int(primary_cfg.get("timeoutMinutes"), 20)
    weak_confidence = _as_float(primary_cfg.get("weakConfidenceThreshold"), 0.55)
    keywords = [str(k).lower() for k in primary_cfg.get("actionableSummaryKeywords", [])]
    enforce_review_state = bool(rollout_settings.get("enforceReviewState", False)) or primary_enforcement != "advisory"

    token = os.getenv(args.token_env, "")
    review_state: ReviewState
    if _as_bool(primary_cfg.get("enabled", True)) and args.repo and token:
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
    elif _as_bool(primary_cfg.get("enabled", True)):
        missing_reason = "review API unavailable (missing repo or token); cannot verify current-head review state"
        review_state = _default_review_state(primary_provider, missing_reason)
    else:
        review_state = ReviewState(
            provider=primary_provider,
            status="missing",
            details=f"provider '{primary_provider}' disabled",
        )

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

    review_states: Dict[str, Dict[str, Any]] = {
        primary_provider: review_state_as_dict(review_state),
    }
    actionable_by_provider: Dict[str, int] = {
        primary_provider: actionable_findings_count,
    }
    secondary_findings_artifacts: Dict[str, str] = {}

    claude_cfg = provider_cfg_map.get("claude", {}) if isinstance(provider_cfg_map.get("claude"), dict) else {}
    claude_enabled = _as_bool(claude_cfg.get("enabled", False))
    claude_enforcement = str(claude_cfg.get("enforcement", "advisory") or "advisory").lower()
    claude_require_ingestion = _as_bool(claude_cfg.get("requireCurrentHeadIngestion", claude_enforcement != "advisory"))

    claude_findings = _load_findings_artifact(args.claude_findings, head_sha=args.head_sha, provider="claude")
    claude_state: ReviewState | None = None
    if claude_enabled:
        claude_state = _review_state_from_findings("claude", claude_findings)
        review_states["claude"] = review_state_as_dict(claude_state)
        claude_actionable_count = count_actionable_findings(claude_findings)
        actionable_by_provider["claude"] = claude_actionable_count
        if Path(args.claude_findings).exists():
            secondary_findings_artifacts["claude"] = args.claude_findings
    else:
        claude_actionable_count = 0

    docs_violations = evaluate_docs_drift(changed_files, contract.get("docsDriftRules", []))

    evidence_needed = requires_browser_evidence(changed_files, contract)
    evidence_errors: List[str] = []
    if evidence_needed:
        evidence_ok, evidence_errors, _ = verify_manifest(
            args.browser_evidence_manifest,
            head_sha=args.head_sha,
            required_flows=evidence_policy.get("required_flows", []),
            required_assertions=evidence_policy.get("required_assertions", []),
            min_screenshots=int(evidence_policy.get("min_screenshots", 2)),
            min_videos=int(evidence_policy.get("min_videos", 1)),
        )
        if evidence_ok:
            evidence_errors = []

    decision = "pass"
    reasons: List[str] = []

    enforce_docs_drift = _as_bool(rollout_settings.get("enforceDocsDrift", False))
    enforce_evidence = _as_bool(rollout_settings.get("enforceEvidence", False))
    enable_remediation = _as_bool(rollout_settings.get("enableRemediation", False))

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
        if primary_enforcement == "advisory":
            record_reason(
                f"{actionable_findings_count} actionable review finding(s) detected",
                enforced=False,
            )
        elif enable_remediation:
            record_reason(f"{actionable_findings_count} actionable review finding(s) detected", enforced=True)
            if decision == "pass":
                decision = "needs-remediation"
        else:
            record_reason(
                f"{actionable_findings_count} actionable review finding(s) detected",
                enforced=True,
            )
            if decision == "pass":
                decision = "fail"

    if claude_enabled and claude_state is not None:
        claude_status = str(claude_state.status or "missing")
    else:
        claude_status = "missing"

    if claude_enabled and claude_status != "success":
        enforced = claude_enforcement != "advisory" or claude_require_ingestion
        record_reason(f"claude review state is '{claude_status}' on current head SHA", enforced=enforced)
        if enforced and decision == "pass":
            decision = "fail"

    if claude_actionable_count > 0:
        message = f"{claude_actionable_count} actionable claude finding(s) detected"
        if claude_enforcement == "advisory":
            record_reason(message, enforced=False)
        elif enable_remediation:
            record_reason(message, enforced=True)
            if decision == "pass":
                decision = "needs-remediation"
        else:
            record_reason(message, enforced=True)
            if decision == "pass":
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

    if not reasons:
        reasons.append("all policy checks passed")

    advisory_actionable_findings_count = 0
    for provider_name, count in actionable_by_provider.items():
        cfg = provider_cfg_map.get(provider_name, {}) if isinstance(provider_cfg_map.get(provider_name), dict) else {}
        enforcement = str(cfg.get("enforcement", "required") or "required").lower()
        if enforcement == "advisory":
            advisory_actionable_findings_count += count

    report = {
        "pr_number": args.pr,
        "head_sha": args.head_sha,
        "risk_tier": risk_tier,
        "changed_files": changed_files,
        "required_checks": required_checks,
        "review_state": review_state_as_dict(review_state),
        "review_states": review_states,
        "actionable_findings_count": actionable_findings_count,
        "actionable_findings_by_provider": actionable_by_provider,
        "advisory_actionable_findings_count": advisory_actionable_findings_count,
        "secondary_findings_artifacts": secondary_findings_artifacts,
        "decision": decision,
        "reasons": reasons,
        "timestamp": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "rollout_phase": rollout_phase,
        "rollout": rollout_settings,
        "review_mode": mode,
        "review_primary": primary_provider,
    }

    _write_json(args.report_out, report)

    enforce_merge_block = _as_bool(rollout_settings.get("enforceMergeBlock", False))
    should_fail_job = enforce_merge_block and decision != "pass"

    print(json.dumps(report, indent=2, sort_keys=True))
    return 1 if should_fail_job else 0


if __name__ == "__main__":
    raise SystemExit(main())
