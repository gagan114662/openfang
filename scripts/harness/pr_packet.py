#!/usr/bin/env python3
"""Build PR acceptance packet with checklist + execution evidence markers."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Dict, List, Tuple

from browser_evidence_verify import verify_manifest


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate PR review packet")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Harness contract path")
    parser.add_argument("--risk-report", default="artifacts/risk-policy-report.json", help="Risk gate report path")
    parser.add_argument("--review-findings", default="artifacts/review-findings.json", help="Primary review findings")
    parser.add_argument("--claude-findings", default="artifacts/claude-findings.json", help="Claude findings")
    parser.add_argument("--browser-evidence-manifest", default="artifacts/browser-evidence-manifest.json", help="Browser evidence manifest path")
    parser.add_argument("--sentry-validation-report", default="artifacts/sentry-logs-validation.json", help="Sentry validation report path")
    parser.add_argument("--out-json", default="artifacts/pr-review-packet.json", help="Output packet JSON")
    parser.add_argument("--out-md", default="artifacts/pr-review-packet.md", help="Output packet markdown")
    return parser.parse_args()


def _read_json(path: str) -> Dict[str, Any]:
    target = Path(path)
    if not target.exists():
        return {}
    try:
        payload = json.loads(target.read_text(encoding="utf-8"))
        return payload if isinstance(payload, dict) else {}
    except Exception:
        return {}


def _write_file(path: str, content: str) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def _provider_ok(payload: Dict[str, Any]) -> Tuple[bool, int, str]:
    status = str(payload.get("status", "missing")).lower()
    findings = payload.get("findings", [])
    actionable_count = 0
    if isinstance(findings, list):
        actionable_count = sum(1 for finding in findings if isinstance(finding, dict) and bool(finding.get("actionable")))
    ok = status == "success" and actionable_count == 0
    return ok, actionable_count, status


def _build_markdown(checklist: List[Dict[str, Any]], media_lines: List[str]) -> str:
    lines: List[str] = []
    lines.append("<!-- pr-review-checklist:start -->")
    lines.append("## PR Acceptance Checklist")
    for item in checklist:
        mark = "x" if item["passed"] else " "
        lines.append(f"- [{mark}] {item['label']} ({item['details']})")
    lines.append("<!-- pr-review-checklist:end -->")
    lines.append("")
    lines.append("<!-- pr-review-media:start -->")
    lines.append("## Execution Evidence")
    lines.extend(media_lines)
    lines.append("<!-- pr-review-media:end -->")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    args = parse_args()

    contract = _read_json(args.contract)
    risk_report = _read_json(args.risk_report)
    review_findings = _read_json(args.review_findings)
    claude_findings = _read_json(args.claude_findings)
    sentry_report = _read_json(args.sentry_validation_report)

    evidence_policy = contract.get("evidencePolicy", {}) if isinstance(contract.get("evidencePolicy", {}), dict) else {}
    head_sha = str(risk_report.get("head_sha", ""))
    risk_decision = str(risk_report.get("decision", "fail")).lower()

    primary_ok, primary_actionable, primary_status = _provider_ok(review_findings)
    claude_ok, claude_actionable, claude_status = _provider_ok(claude_findings)

    browser_ok = True
    browser_details = "not required"
    if Path(args.browser_evidence_manifest).exists():
        browser_ok, browser_errors, _ = verify_manifest(
            args.browser_evidence_manifest,
            head_sha=head_sha or None,
            required_flows=evidence_policy.get("requiredFlows", []),
            required_assertions=evidence_policy.get("requiredAssertions", []),
        )
        browser_details = "ok" if browser_ok else "; ".join(browser_errors)

    sentry_required = "sentry-live-validate" in risk_report.get("required_checks", [])
    sentry_status = str(sentry_report.get("status", "missing")).lower()
    sentry_ok = (not sentry_required) or (sentry_status == "pass" and bool(sentry_report.get("ok", False)))

    checklist = [
        {
            "id": "risk_policy_pass",
            "label": "Risk policy gate is green on current head",
            "passed": risk_decision == "pass",
            "details": risk_decision,
        },
        {
            "id": "greptile_clean",
            "label": "Greptile/provider review clean",
            "passed": primary_ok,
            "details": f"status={primary_status}, actionable={primary_actionable}",
        },
        {
            "id": "claude_clean",
            "label": "Claude review clean",
            "passed": claude_ok,
            "details": f"status={claude_status}, actionable={claude_actionable}",
        },
        {
            "id": "browser_evidence",
            "label": "Browser execution evidence verified",
            "passed": browser_ok,
            "details": browser_details,
        },
        {
            "id": "sentry_live_validation",
            "label": "Sentry live validation passed",
            "passed": sentry_ok,
            "details": "not required" if not sentry_required else sentry_status,
        },
    ]

    all_passed = all(item["passed"] for item in checklist)

    media_lines = [
        f"- Browser evidence manifest: `{args.browser_evidence_manifest}`",
        f"- Sentry live validation report: `{args.sentry_validation_report}`",
        "- Embedded browser flow proof should be attached as artifact and linked from this PR comment.",
    ]

    packet = {
        "head_sha": head_sha,
        "pr_number": risk_report.get("pr_number"),
        "risk_tier": risk_report.get("risk_tier"),
        "required_checks": risk_report.get("required_checks", []),
        "review_providers": {
            "primary": {
                "status": primary_status,
                "actionable": primary_actionable,
            },
            "claude": {
                "status": claude_status,
                "actionable": claude_actionable,
            },
        },
        "browser_evidence": {
            "ok": browser_ok,
            "manifest": args.browser_evidence_manifest,
        },
        "sentry_live_validation": {
            "required": sentry_required,
            "status": sentry_status,
            "ok": sentry_ok,
        },
        "checklist": checklist,
        "all_passed": all_passed,
    }

    markdown = _build_markdown(checklist, media_lines)
    _write_file(args.out_json, json.dumps(packet, indent=2, sort_keys=True) + "\n")
    _write_file(args.out_md, markdown)

    print(json.dumps(packet, indent=2, sort_keys=True))
    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
