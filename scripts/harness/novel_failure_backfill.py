#!/usr/bin/env python3
"""Generate candidate eval scenarios from novel failure findings."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Tuple


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Backfill candidate scenarios from findings")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Policy contract path")
    parser.add_argument("--sentry-findings", default="artifacts/sentry-findings.json", help="Sentry findings path")
    parser.add_argument("--eval-findings", default="artifacts/agent-evals/eval-findings.json", help="Eval findings path")
    parser.add_argument("--review-findings", default="artifacts/review-findings.json", help="Greptile findings path")
    parser.add_argument("--claude-findings", default="artifacts/claude-findings.json", help="Claude findings path")
    parser.add_argument("--head-sha", default="", help="Head SHA (optional)")
    parser.add_argument("--out", default="artifacts/agent-evals/novel-failure-candidates.json", help="Output candidates report")
    parser.add_argument("--target-scenarios", default="", help="Override target generated scenarios file path")
    parser.add_argument("--dedupe-file", default="", help="Override dedupe signatures file path")
    return parser.parse_args()


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


def _git_head_sha() -> str:
    try:
        proc = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True)
        return proc.stdout.strip()
    except Exception:
        return ""


def _normalize_path(path: str) -> str:
    value = path.strip().replace("\\", "/")
    while value.startswith("./"):
        value = value[2:]
    return value


def _surface_for_path(path: str) -> str:
    p = _normalize_path(path)
    if p.startswith("crates/openfang-api/"):
        return "api"
    if p.startswith("crates/openfang-runtime/"):
        return "runtime"
    if p.startswith("crates/openfang-kernel/"):
        return "kernel"
    if p.startswith("crates/openfang-channels/"):
        return "channels"
    if p.startswith("crates/openfang-cli/"):
        return "cli"
    return "harness"


def _scenario_id(failure_class: str, signature: str) -> str:
    klass = re.sub(r"[^a-z0-9]+", "-", failure_class.lower()).strip("-") or "unknown"
    return f"generated-{klass}-{signature[:12]}"


def _fingerprint(provider: str, finding: Dict[str, Any]) -> str:
    payload = {
        "provider": provider,
        "failure_class": str(finding.get("failure_class", "")).strip().lower(),
        "path": _normalize_path(str(finding.get("path", ""))),
        "summary": str(finding.get("summary", "")).strip().lower()[:240],
        "severity": str(finding.get("severity", "medium")).strip().lower(),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _candidate_scenario(provider: str, finding: Dict[str, Any], signature: str) -> Dict[str, Any]:
    path = _normalize_path(str(finding.get("path", "")).strip()) or "docs/harness-engineering.md"
    failure_class = str(finding.get("failure_class", "")).strip().lower() or "novel_failure"
    summary = str(finding.get("summary", "")).strip() or "Novel failure candidate"
    severity = str(finding.get("severity", "medium")).strip().lower() or "medium"
    line = int(finding.get("line", 1) or 1)
    if line < 1:
        line = 1

    return {
        "id": _scenario_id(failure_class, signature),
        "name": f"Generated candidate: {summary[:80]}",
        "tier": "nightly",
        "surface": _surface_for_path(path),
        "setup": {"component": "generated-backfill", "mode": "candidate"},
        "stimulus": {"kind": "source_scan", "path": path},
        "expected": {"kind": "file_exists", "path": path},
        "judge": {"kind": "file_exists", "path": path},
        "failure_class": failure_class,
        "remediable": False,
        "owner": "harness",
        "timeout_secs": 20,
        "severity": severity if severity in {"critical", "high", "medium", "low", "info"} else "medium",
        "candidate_only": True,
        "generated_from": {
            "provider": provider,
            "summary": summary,
            "path": path,
            "line": line,
            "signature": signature,
        },
    }


def _collect_findings(path: Path, provider_hint: str) -> Tuple[str, List[Dict[str, Any]], List[str]]:
    payload = _read_json(path, {})
    if not payload:
        return provider_hint, [], []
    provider = str(payload.get("provider", provider_hint)).strip().lower() or provider_hint
    status = str(payload.get("status", "")).strip().lower()
    errors = payload.get("errors", [])
    errors = [str(item) for item in errors] if isinstance(errors, list) else []
    if status not in {"success", "pass"}:
        return provider, [], errors
    findings = payload.get("findings", [])
    findings = [item for item in findings if isinstance(item, dict)] if isinstance(findings, list) else []
    return provider, findings, errors


def main() -> int:
    args = parse_args()
    contract = _read_json(Path(args.contract), {})
    eval_policy = contract.get("agentEvalPolicy", {}) if isinstance(contract.get("agentEvalPolicy"), dict) else {}
    backfill_cfg = eval_policy.get("novelFailureBackfill", {}) if isinstance(eval_policy.get("novelFailureBackfill"), dict) else {}

    enabled = bool(backfill_cfg.get("enabled", True))
    max_new = int(backfill_cfg.get("maxNewScenariosPerRun", 25) or 25)
    min_conf = float(backfill_cfg.get("minConfidence", 0.7) or 0.7)
    target_file = args.target_scenarios or str(backfill_cfg.get("targetScenarioFile", ".harness/evals/scenarios.generated.json"))
    dedupe_file = args.dedupe_file or str(backfill_cfg.get("dedupeFile", ".harness/evals/novel-failure-signatures.json"))

    report: Dict[str, Any] = {
        "status": "missing",
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "head_sha": args.head_sha or _git_head_sha(),
        "new_candidates": 0,
        "deduped_candidates": 0,
        "candidate_scenarios": [],
        "errors": [],
    }

    if not enabled:
        report["status"] = "success"
        report["errors"].append("novelFailureBackfill.enabled is false")
        _write_json(Path(args.out), report)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0

    target_path = Path(target_file)
    dedupe_path = Path(dedupe_file)

    target_payload = _read_json(
        target_path,
        {
            "version": "1.0.0",
            "description": "Generated candidate scenarios mined from novel production failures. Candidate-only by default.",
            "profile": "nightly-generated",
            "generated_at": "",
            "scenarios": [],
        },
    )
    existing_scenarios = target_payload.get("scenarios", [])
    if not isinstance(existing_scenarios, list):
        existing_scenarios = []
    existing_ids = {
        str(item.get("id"))
        for item in existing_scenarios
        if isinstance(item, dict) and str(item.get("id", "")).strip()
    }

    dedupe_payload = _read_json(dedupe_path, {"version": "1.0.0", "description": "", "signatures": []})
    signatures_raw = dedupe_payload.get("signatures", [])
    signatures = {str(item) for item in signatures_raw if str(item).strip()} if isinstance(signatures_raw, list) else set()

    sources = [
        (Path(args.sentry_findings), "sentry"),
        (Path(args.eval_findings), "eval"),
        (Path(args.review_findings), "greptile"),
        (Path(args.claude_findings), "claude"),
    ]

    candidates: List[Dict[str, Any]] = []
    for source_path, provider_hint in sources:
        provider, findings, errors = _collect_findings(source_path, provider_hint)
        report["errors"].extend(errors)
        for finding in findings:
            if not bool(finding.get("actionable", False)):
                continue
            confidence = float(finding.get("confidence", 0) or 0)
            if confidence < min_conf:
                continue
            signature = _fingerprint(provider, finding)
            if signature in signatures:
                report["deduped_candidates"] += 1
                continue
            scenario = _candidate_scenario(provider, finding, signature)
            if scenario["id"] in existing_ids:
                report["deduped_candidates"] += 1
                signatures.add(signature)
                continue
            candidates.append(scenario)
            signatures.add(signature)
            existing_ids.add(str(scenario["id"]))
            if len(candidates) >= max_new:
                break
        if len(candidates) >= max_new:
            break

    candidates.sort(key=lambda item: str(item.get("id", "")))
    all_scenarios = list(existing_scenarios) + candidates
    all_scenarios.sort(key=lambda item: str(item.get("id", "")) if isinstance(item, dict) else "")

    target_payload["generated_at"] = dt.datetime.now(tz=dt.timezone.utc).isoformat()
    target_payload["scenarios"] = all_scenarios
    dedupe_payload["signatures"] = sorted(signatures)

    _write_json(target_path, target_payload)
    _write_json(dedupe_path, dedupe_payload)

    report["status"] = "success"
    report["new_candidates"] = len(candidates)
    report["candidate_scenarios"] = candidates
    _write_json(Path(args.out), report)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
