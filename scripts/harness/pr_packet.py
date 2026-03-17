#!/usr/bin/env python3
"""Build acceptance checklist and PR review packet artifacts."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Tuple

from checks_resolver import any_path_matches, load_contract, normalize_path, read_changed_files


def _read_json(path: Path, default: Dict[str, Any]) -> Dict[str, Any]:
    if not path.exists():
        return default
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return default
    return payload if isinstance(payload, dict) else default


def _write_json(path: Path, payload: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _write_text(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def _to_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "enabled", "on"}
    return False


def _count_artifact_kind(artifacts: List[Dict[str, Any]], kind: str) -> int:
    return len([item for item in artifacts if str(item.get("kind", "")).lower() == kind])


def _check_runs(repo: str, sha: str, token: str) -> List[Dict[str, Any]]:
    if not repo or not sha or not token:
        return []
    url = f"https://api.github.com/repos/{repo}/commits/{sha}/check-runs?per_page=100"
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Authorization": f"Bearer {token}",
    }
    req = urllib.request.Request(url, headers=headers, method="GET")
    with urllib.request.urlopen(req, timeout=30) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
        runs = payload.get("check_runs", [])
        return runs if isinstance(runs, list) else []


def _criterion_map(acceptance_model: Dict[str, Any]) -> Dict[str, Dict[str, str]]:
    items = {}
    for entry in acceptance_model.get("core", []):
        if isinstance(entry, dict) and entry.get("id"):
            items[str(entry["id"])] = {
                "title": str(entry.get("title", entry["id"])),
                "description": str(entry.get("description", "")),
            }
    for rule in acceptance_model.get("path_rules", []):
        if not isinstance(rule, dict):
            continue
        for criterion_id in rule.get("criteria", []):
            if criterion_id not in items:
                items[str(criterion_id)] = {
                    "title": str(criterion_id),
                    "description": "",
                }
    return items


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate PR acceptance packet")
    parser.add_argument("--acceptance-model", default=".harness/acceptance.criteria.json", help="Acceptance model JSON path")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Harness contract JSON path")
    parser.add_argument("--changed-files", required=True, help="changed_files.txt path")
    parser.add_argument("--risk-report", required=True, help="risk-policy-report.json path")
    parser.add_argument(
        "--infra-preflight-report",
        default="artifacts/infra-preflight-report.json",
        help="infra-preflight-report.json path",
    )
    parser.add_argument(
        "--live-provider-report",
        default="artifacts/agent-evals/live-provider-report.json",
        help="live-provider-report.json path",
    )
    parser.add_argument(
        "--eval-results",
        default="artifacts/agent-evals/eval-results.json",
        help="eval-results.json path",
    )
    parser.add_argument(
        "--claude-findings",
        default="artifacts/claude-findings.json",
        help="Optional claude-findings.json path",
    )
    parser.add_argument(
        "--sentry-validation-report",
        default="artifacts/sentry-logs-validation.json",
        help="sentry live validation report path",
    )
    parser.add_argument("--evidence-manifest", required=True, help="browser-evidence-manifest.json path")
    parser.add_argument("--head-sha", required=True, help="Head SHA under review")
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="owner/repo")
    parser.add_argument("--pr-number", type=int, default=0, help="PR number for context")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Token env var for check-run lookups")
    parser.add_argument("--out-dir", default="artifacts/pr-review", help="Output directory")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    acceptance_model = _read_json(Path(args.acceptance_model), {})
    contract = load_contract(args.contract)
    changed_files = read_changed_files(args.changed_files)
    risk_report = _read_json(Path(args.risk_report), {})
    infra_preflight_report = _read_json(Path(args.infra_preflight_report), {})
    live_provider_report = _read_json(Path(args.live_provider_report), {})
    eval_results = _read_json(Path(args.eval_results), {})
    claude_findings = _read_json(Path(args.claude_findings), {})
    sentry_validation = _read_json(Path(args.sentry_validation_report), {})
    evidence = _read_json(Path(args.evidence_manifest), {})

    criterion_meta = _criterion_map(acceptance_model)
    core_ids = [str(item["id"]) for item in acceptance_model.get("core", []) if isinstance(item, dict) and item.get("id")]
    active_ids = list(core_ids)

    for rule in acceptance_model.get("path_rules", []):
        if not isinstance(rule, dict):
            continue
        patterns = [str(item) for item in rule.get("when_touched", []) if str(item).strip()]
        if patterns and any_path_matches(changed_files, patterns):
            for cid in rule.get("criteria", []):
                value = str(cid)
                if value not in active_ids:
                    active_ids.append(value)

    artifacts = evidence.get("artifacts", [])
    artifacts = artifacts if isinstance(artifacts, list) else []
    assertions = evidence.get("assertions", [])
    assertions = assertions if isinstance(assertions, list) else []
    assertion_status = {
        str(item.get("name", "")): str(item.get("status", "")).lower()
        for item in assertions
        if isinstance(item, dict)
    }

    screenshot_count = _count_artifact_kind([item for item in artifacts if isinstance(item, dict)], "screenshot")
    video_count = _count_artifact_kind([item for item in artifacts if isinstance(item, dict)], "video")

    harness_cfg = contract.get("prReviewHarness", {}) if isinstance(contract.get("prReviewHarness"), dict) else {}
    max_files = int(harness_cfg.get("maxFiles", 200))
    min_screenshots = int(harness_cfg.get("minScreenshots", 2))
    min_videos = int(harness_cfg.get("minVideos", 1))
    required_check_name = str(harness_cfg.get("requiredCheckName", "pr-review-harness"))
    review_providers = contract.get("reviewProviders", {})
    providers_map = review_providers.get("providers", {}) if isinstance(review_providers, dict) else {}
    greptile_cfg = providers_map.get("greptile", {}) if isinstance(providers_map.get("greptile"), dict) else {}
    legacy_review_policy = contract.get("reviewPolicy", {}) if isinstance(contract.get("reviewPolicy"), dict) else {}
    greptile_check_name = str(
        greptile_cfg.get("checkRunName", legacy_review_policy.get("checkRunName", "greptile-review"))
    )

    required_checks = risk_report.get("required_checks", [])
    required_checks = [str(item) for item in required_checks] if isinstance(required_checks, list) else []
    decision = str(risk_report.get("decision", "unknown"))
    review_primary = str(risk_report.get("review_primary", "greptile") or "greptile")
    review_states = risk_report.get("review_states", {})
    review_states = review_states if isinstance(review_states, dict) else {}
    greptile_report_state = review_states.get("greptile", {})
    if not isinstance(greptile_report_state, dict):
        greptile_report_state = {}
    claude_report_state = review_states.get("claude", {})
    if not isinstance(claude_report_state, dict):
        claude_report_state = {}
    actionable_by_provider = risk_report.get("actionable_findings_by_provider", {})
    actionable_by_provider = actionable_by_provider if isinstance(actionable_by_provider, dict) else {}
    greptile_actionable = int(actionable_by_provider.get("greptile", 0) or 0)
    claude_actionable_from_report = int(actionable_by_provider.get("claude", 0) or 0)
    legacy_review_state = risk_report.get("review_state", {})
    if (
        not greptile_report_state
        and isinstance(legacy_review_state, dict)
        and str(legacy_review_state.get("provider", "")).lower() == "greptile"
    ):
        state = legacy_review_state
        greptile_report_state = state if isinstance(state, dict) else {}

    check_runs = _check_runs(args.repo, args.head_sha, os.getenv(args.token_env, ""))
    check_runs_by_name: Dict[str, List[Dict[str, Any]]] = {}
    for run in check_runs:
        if not isinstance(run, dict):
            continue
        name = str(run.get("name", ""))
        check_runs_by_name.setdefault(name, []).append(run)

    greptile_runs = check_runs_by_name.get(greptile_check_name, [])
    greptile_run = (
        sorted(greptile_runs, key=lambda item: int(item.get("id", 0)), reverse=True)[0]
        if greptile_runs
        else {}
    )
    greptile_run_status = str(greptile_run.get("status", "") or "missing")
    greptile_run_conclusion = str(greptile_run.get("conclusion", "") or "n/a")
    greptile_report_status = str(greptile_report_state.get("status", "") or "missing")
    claude_report_status = str(claude_report_state.get("status", "") or "missing")
    greptile_report_url = str(greptile_report_state.get("check_run_url", "") or "").strip()
    greptile_run_url = str(greptile_run.get("html_url", "") or "").strip()
    greptile_url = greptile_report_url or greptile_run_url

    def verify_diff_scope() -> Tuple[bool, str]:
        changed_count = len(changed_files)
        if changed_count == 0:
            return False, "no changed files detected for PR head"
        if changed_count > max_files:
            return False, f"changed files {changed_count} exceeds maxFiles {max_files}"
        return True, f"{changed_count} changed files within maxFiles={max_files}"

    def verify_required_ci_signals() -> Tuple[bool, str]:
        failures = []
        details = []
        non_checkrun_requirements = {"sentry-live-validate"}
        for check in required_checks:
            if check == required_check_name:
                continue
            if check in non_checkrun_requirements:
                details.append(f"{check}=covered-via-artifact")
                continue
            runs = check_runs_by_name.get(check, [])
            if not runs:
                details.append(f"{check}=missing")
                failures.append(check)
                continue
            run = sorted(runs, key=lambda item: int(item.get("id", 0)), reverse=True)[0]
            status = str(run.get("status", ""))
            conclusion = str(run.get("conclusion", "") or "")
            details.append(f"{check}={status}/{conclusion or 'n/a'}")
            if conclusion.lower() in {"failure", "cancelled", "timed_out", "action_required"}:
                failures.append(check)
        if failures:
            return False, f"missing or failing required checks: {', '.join(sorted(set(failures)))}"
        if details:
            return True, "; ".join(details)
        return True, "no non-harness required CI checks declared for this risk tier"

    def verify_evidence_package() -> Tuple[bool, str]:
        if not artifacts:
            return False, "manifest contains zero artifacts"
        return True, f"manifest contains {len(artifacts)} artifacts"

    def verify_minimum_screenshots() -> Tuple[bool, str]:
        if screenshot_count < min_screenshots:
            return False, f"{screenshot_count} screenshots < required {min_screenshots}"
        return True, f"{screenshot_count} screenshots >= required {min_screenshots}"

    def verify_minimum_videos() -> Tuple[bool, str]:
        if video_count < min_videos:
            return False, f"{video_count} videos < required {min_videos}"
        return True, f"{video_count} videos >= required {min_videos}"

    def verify_policy_state() -> Tuple[bool, str]:
        ok = decision == "pass"
        return (ok, f"risk gate decision: {decision}")

    def verify_review_providers_clean() -> Tuple[bool, str]:
        greptile_ok = greptile_report_status == "success" and greptile_actionable == 0
        claude_ok = claude_report_status == "success" and claude_actionable_from_report == 0
        details = (
            f"greptile(status={greptile_report_status}, actionable={greptile_actionable}); "
            f"claude(status={claude_report_status}, actionable={claude_actionable_from_report})"
        )
        return (greptile_ok and claude_ok, details)

    def verify_infra_preflight_pass() -> Tuple[bool, str]:
        status = str(infra_preflight_report.get("status", "")).lower()
        ok = status == "pass"
        return (ok, f"infra preflight status: {status or 'missing'}")

    def verify_live_provider_gate_pass() -> Tuple[bool, str]:
        status = str(live_provider_report.get("status", "")).lower()
        ok = status == "pass"
        return (ok, f"live provider probe status: {status or 'missing'}")

    def verify_agent_blocking_evals_pass() -> Tuple[bool, str]:
        summary = eval_results.get("summary", {})
        summary = summary if isinstance(summary, dict) else {}
        failed = int(summary.get("failed", 0) or 0)
        all_blocking_passed = _to_bool(summary.get("all_blocking_passed", failed == 0))
        return (all_blocking_passed, f"blocking evals failed={failed}")

    def verify_sentry_live_validation_pass() -> Tuple[bool, str]:
        if "sentry-live-validate" not in set(required_checks):
            return (True, "sentry live validation not required for this risk tier")
        status = str(sentry_validation.get("status", "")).lower()
        errors = sentry_validation.get("errors", [])
        errors_count = len(errors) if isinstance(errors, list) else 0
        ok = status == "success"
        return (ok, f"sentry live validation status={status or 'missing'}, errors={errors_count}")

    def verify_ui_evidence_assertions() -> Tuple[bool, str]:
        shot = assertion_status.get("ui_screenshot_evidence", "")
        vid = assertion_status.get("ui_video_evidence", "")
        ok = shot == "pass" and vid == "pass"
        return (ok, f"ui_screenshot_evidence={shot or 'missing'}, ui_video_evidence={vid or 'missing'}")

    def verify_api_runtime_validation() -> Tuple[bool, str]:
        needed = {"ci-check", "ci-test"}
        present = set(required_checks)
        missing = sorted(needed - present)
        if missing:
            return False, f"required checks missing for API/runtime scope: {', '.join(missing)}"
        return True, "api/runtime scope required checks present (ci-check, ci-test)"

    def verify_docs_consistency() -> Tuple[bool, str]:
        docs_patterns = ["docs/**", "*.md", "*.toml.example"]
        touched = any_path_matches(changed_files, docs_patterns)
        if touched:
            return True, "docs/config files touched in this PR"
        return False, "docs/config criterion active but no matching files found"

    verifier = {
        "diff_scoped_coherent": verify_diff_scope,
        "required_ci_signals_green": verify_required_ci_signals,
        "evidence_package_exists": verify_evidence_package,
        "minimum_screenshots": verify_minimum_screenshots,
        "minimum_videos": verify_minimum_videos,
        "no_harness_policy_violations": verify_policy_state,
        "review_providers_clean": verify_review_providers_clean,
        "ui_evidence_assertions_pass": verify_ui_evidence_assertions,
        "api_runtime_validation_present": verify_api_runtime_validation,
        "docs_consistency_reviewed": verify_docs_consistency,
        "infra_preflight_pass": verify_infra_preflight_pass,
        "live_provider_gate_pass": verify_live_provider_gate_pass,
        "agent_blocking_evals_pass": verify_agent_blocking_evals_pass,
        "sentry_live_validation_pass": verify_sentry_live_validation_pass,
    }

    criteria_rows: List[Dict[str, Any]] = []
    all_passed = True
    for cid in active_ids:
        run = verifier.get(cid)
        title = criterion_meta.get(cid, {}).get("title", cid)
        if not run:
            passed, details = False, "missing verifier implementation"
        else:
            passed, details = run()
        all_passed = all_passed and passed
        criteria_rows.append(
            {
                "id": cid,
                "title": title,
                "passed": passed,
                "details": details,
            }
        )

    evidence_inventory = []
    for item in artifacts:
        if not isinstance(item, dict):
            continue
        evidence_inventory.append(
            {
                "kind": str(item.get("kind", "")),
                "path": str(item.get("path", "")),
                "sha256": str(item.get("sha256", "")),
                "size_bytes": int(item.get("size_bytes", 0)),
            }
        )

    checklist_payload = {
        "head_sha": args.head_sha,
        "pr_number": args.pr_number,
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "all_passed": all_passed,
        "active_criteria_count": len(criteria_rows),
        "criteria": criteria_rows,
        "evidence_inventory": evidence_inventory,
        "review_providers": {
            "primary": review_primary,
            "greptile": {
                "check_run_name": greptile_check_name,
                "report_status": greptile_report_status,
                "actionable_findings": greptile_actionable,
                "check_run_status": greptile_run_status,
                "check_run_conclusion": greptile_run_conclusion,
                "check_run_url": greptile_url,
            },
            "claude": {
                "report_status": claude_report_status,
                "actionable_findings": claude_actionable_from_report,
            },
        },
        "sentry_live_validation": {
            "status": str(sentry_validation.get("status", "") or "missing"),
            "errors": sentry_validation.get("errors", []),
        },
    }

    claude_provider = str(claude_findings.get("provider", "")).strip().lower()
    claude_status = str(claude_findings.get("status", "")).strip().lower()
    claude_items = claude_findings.get("findings", [])
    claude_items = [item for item in claude_items if isinstance(item, dict)] if isinstance(claude_items, list) else []
    claude_actionable = [item for item in claude_items if bool(item.get("actionable", False))]
    claude_severity_counts: Dict[str, int] = {}
    for item in claude_items:
        severity = str(item.get("severity", "info")).lower()
        claude_severity_counts[severity] = claude_severity_counts.get(severity, 0) + 1

    md_lines = [
        "## PR Review Harness Checklist",
        "",
        f"- Head SHA: `{args.head_sha}`",
        f"- Overall status: {'PASS' if all_passed else 'FAIL'}",
        "",
        "### Acceptance Criteria",
    ]
    for row in criteria_rows:
        marker = "x" if row["passed"] else " "
        md_lines.append(f"- [{marker}] **{row['title']}** (`{row['id']}`)")
        md_lines.append(f"  - {row['details']}")

    md_lines.extend(
        [
            "",
            "### Review Providers",
            "",
            f"- Primary provider: `{review_primary}`",
            f"- Greptile check run: `{greptile_check_name}`",
            f"- Greptile report state: `{greptile_report_status}`",
            f"- Greptile check-run state: `{greptile_run_status}/{greptile_run_conclusion}`",
            f"- Greptile actionable findings: `{greptile_actionable}`",
            f"- Claude report state: `{claude_report_status}`",
            f"- Claude actionable findings: `{claude_actionable_from_report}`",
        ]
    )
    if greptile_url:
        md_lines.append(f"- Greptile link: {greptile_url}")

    md_lines.extend(
        [
            "",
            "### Evidence Inventory",
            "",
            "| Type | Path | Size (bytes) | SHA256 |",
            "| --- | --- | ---: | --- |",
        ]
    )
    for item in evidence_inventory:
        md_lines.append(
            f"| {item['kind']} | `{item['path']}` | {item['size_bytes']} | `{item['sha256']}` |"
        )
    if not evidence_inventory:
        md_lines.append("| n/a | n/a | 0 | n/a |")

    if claude_provider == "claude":
        md_lines.extend(
            [
                "",
                "### Claude Feedback",
                "",
                f"- Provider status: `{claude_status or 'missing'}`",
                f"- Total findings: `{len(claude_items)}`",
                f"- Actionable findings: `{len(claude_actionable)}`",
            ]
        )
        if claude_severity_counts:
            sev_parts = [f"{key}:{claude_severity_counts[key]}" for key in sorted(claude_severity_counts.keys())]
            md_lines.append(f"- Severity counts: `{', '.join(sev_parts)}`")
        if claude_actionable:
            md_lines.append("- Top actionable items:")
            for item in claude_actionable[:5]:
                path = str(item.get("path", "")).strip() or "unknown"
                line = int(item.get("line", 1) or 1)
                summary = str(item.get("summary", "")).strip() or "(no summary)"
                md_lines.append(f"  - `{path}:{line}` {summary}")
        errors = claude_findings.get("errors", [])
        if isinstance(errors, list) and errors:
            md_lines.append(f"- Ingestion errors: `{len(errors)}` (see `claude-findings.json` artifact)")

    sentry_status = str(sentry_validation.get("status", "") or "missing")
    sentry_errors = sentry_validation.get("errors", [])
    sentry_error_count = len(sentry_errors) if isinstance(sentry_errors, list) else 0
    md_lines.extend(
        [
            "",
            "### Sentry Live Validation",
            "",
            f"- Status: `{sentry_status}`",
            f"- Errors: `{sentry_error_count}`",
        ]
    )

    checklist_md = "\n".join(md_lines) + "\n"
    comment_md = (
        "<!-- pr-review-harness -->\n"
        "### PR Review Harness\n\n"
        f"Status: **{'PASS' if all_passed else 'FAIL'}**\n\n"
        "Review evidence artifacts are attached to this workflow run.\n\n"
        + checklist_md
    )
    body_block = (
        "<!-- pr-review-checklist:start -->\n"
        + checklist_md
        + "<!-- pr-review-checklist:end -->\n"
    )

    _write_json(out_dir / "acceptance-checklist.json", checklist_payload)
    _write_text(out_dir / "acceptance-checklist.md", checklist_md)
    _write_text(out_dir / "pr-comment.md", comment_md)
    _write_text(out_dir / "pr-body-block.md", body_block)

    print(json.dumps({"ok": True, "all_passed": all_passed, "out_dir": str(out_dir)}, indent=2))
    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
