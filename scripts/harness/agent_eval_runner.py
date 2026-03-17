#!/usr/bin/env python3
"""Deterministic agent eval runner for OpenFang."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
from typing import Any, Dict, List, Tuple

from agent_eval_judges import evaluate_judge
from checks_resolver import any_path_matches, read_changed_files


def _read_json(path: Path) -> Dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"expected object in {path}")
    return payload


def _write_json(path: Path, payload: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _write_text(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def _hash_result(item: Dict[str, Any]) -> str:
    stable = {
        "scenario_id": item.get("scenario_id"),
        "seed": item.get("seed"),
        "pass": item.get("pass"),
        "observed": item.get("observed"),
        "expected": item.get("expected"),
        "failure_reason": item.get("failure_reason"),
    }
    encoded = json.dumps(stable, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _severity_rank(value: str) -> int:
    order = {"critical": 0, "high": 1, "medium": 2, "low": 3, "info": 4}
    return order.get(value.lower(), 5)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run deterministic OpenFang agent evals")
    parser.add_argument("--scenarios", required=True, help="Scenario corpus JSON path")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Policy contract path")
    parser.add_argument("--head-sha", required=True, help="Head SHA under evaluation")
    parser.add_argument("--profile", default="blocking", choices=["blocking", "nightly"], help="Eval profile")
    parser.add_argument("--seed", type=int, default=42, help="Deterministic seed")
    parser.add_argument("--changed-files", default="", help="Optional changed_files.txt path")
    parser.add_argument(
        "--generated-scenarios",
        default="",
        help="Optional generated scenarios JSON path to merge into scenario corpus",
    )
    parser.add_argument("--repo-root", default=".", help="Repository root path")
    parser.add_argument("--out-dir", default="artifacts/agent-evals", help="Output directory")
    parser.add_argument("--max-scenario-runtime-secs", type=int, default=60, help="Max runtime per scenario")
    parser.add_argument(
        "--enforce",
        default="auto",
        choices=["auto", "true", "false"],
        help="Whether to return non-zero on blocker failure",
    )
    return parser.parse_args()


def _enforce_enabled(profile: str, flag: str) -> bool:
    if flag == "true":
        return True
    if flag == "false":
        return False
    return profile == "blocking"


def _scenario_applies(scenario: Dict[str, Any], changed_files: List[str]) -> bool:
    patterns = scenario.get("when_touched", [])
    if not patterns:
        return True
    if not changed_files:
        return True
    text_patterns = [str(item) for item in patterns if str(item).strip()]
    if not text_patterns:
        return True
    return any_path_matches(changed_files, text_patterns)


def _normalize_finding(
    scenario: Dict[str, Any],
    result: Dict[str, Any],
) -> Dict[str, Any]:
    finding_cfg = scenario.get("finding", {}) if isinstance(scenario.get("finding"), dict) else {}
    severity = str(finding_cfg.get("severity") or scenario.get("severity") or "medium").lower()
    if severity not in {"critical", "high", "medium", "low", "info"}:
        severity = "medium"

    path = str(finding_cfg.get("path") or scenario.get("stimulus", {}).get("path") or "").strip()
    if not path:
        path = "unknown"

    line = int(result.get("line_guess", finding_cfg.get("line", 1)) or 1)
    if line < 1:
        line = 1

    summary = str(
        finding_cfg.get("summary")
        or f"Eval scenario '{scenario.get('id', 'unknown')}' failed: {result.get('failure_reason', 'assertion failed')}"
    ).strip()

    return {
        "id": f"eval-{scenario.get('id', 'unknown')}",
        "severity": severity,
        "confidence": 0.95,
        "path": path,
        "line": line,
        "summary": summary,
        "actionable": True,
        "scenario_id": str(scenario.get("id", "")),
        "failure_class": str(scenario.get("failure_class", "")),
    }


def _build_summary_markdown(
    *,
    head_sha: str,
    profile: str,
    seed: int,
    summary: Dict[str, Any],
    results: List[Dict[str, Any]],
) -> str:
    lines = [
        "# OpenFang Agent Evals",
        "",
        f"- Head SHA: `{head_sha}`",
        f"- Profile: `{profile}`",
        f"- Seed: `{seed}`",
        f"- Total: `{summary['total']}`",
        f"- Passed: `{summary['passed']}`",
        f"- Failed: `{summary['failed']}`",
        f"- Pass rate: `{summary['pass_rate']:.3f}`",
        f"- Blocking threshold: `{summary['blocking_threshold']:.3f}`",
        f"- All blocking passed: `{summary['all_blocking_passed']}`",
        "",
        "## Failure Classes",
        "",
    ]

    failure_classes = summary.get("failure_classes", {})
    if failure_classes:
        for klass, count in sorted(failure_classes.items()):
            lines.append(f"- `{klass}`: {count}")
    else:
        lines.append("- none")

    lines.extend(
        [
            "",
            "## Scenario Results",
            "",
            "| Scenario | Surface | Failure Class | Pass | Duration (ms) | Reason |",
            "| --- | --- | --- | --- | ---: | --- |",
        ]
    )

    for item in results:
        reason = str(item.get("failure_reason", "")).replace("|", "/")
        lines.append(
            "| {sid} | {surface} | {klass} | {ok} | {ms} | {reason} |".format(
                sid=item.get("scenario_id", ""),
                surface=item.get("surface", ""),
                klass=item.get("failure_class", ""),
                ok="PASS" if bool(item.get("pass")) else "FAIL",
                ms=item.get("duration_ms", 0),
                reason=reason or "-",
            )
        )

    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()

    repo_root = Path(args.repo_root).resolve()
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    scenarios_payload = _read_json(Path(args.scenarios))
    contract = _read_json(Path(args.contract))

    policy = contract.get("agentEvalPolicy", {})
    if not isinstance(policy, dict):
        policy = {}

    enabled = bool(policy.get("enabled", True))
    if not enabled:
        empty_results = {
            "head_sha": args.head_sha,
            "profile": args.profile,
            "seed": args.seed,
            "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
            "summary": {
                "total": 0,
                "passed": 0,
                "failed": 0,
                "pass_rate": 1.0,
                "blocking_threshold": float(policy.get("blockingThreshold", 1.0) or 1.0),
                "all_blocking_passed": True,
                "failure_classes": {},
            },
            "results": [],
            "profile_config": policy.get("blockingProfile" if args.profile == "blocking" else "nightlyProfile", {}),
        }
        findings = {
            "head_sha": args.head_sha,
            "provider": "eval",
            "status": "success",
            "summary": {"total_failed": 0, "actionable": 0},
            "findings": [],
            "errors": ["agentEvalPolicy.enabled is false"],
        }
        _write_json(out_dir / "eval-results.json", empty_results)
        _write_json(out_dir / "eval-findings.json", findings)
        _write_text(out_dir / "eval-summary.md", "# OpenFang Agent Evals\n\nAgent evals are disabled by policy.\n")
        print(json.dumps({"ok": True, "disabled": True, "out_dir": str(out_dir)}, indent=2))
        return 0

    if "scenarios" not in scenarios_payload:
        raise ValueError("scenarios file missing required key 'scenarios'")
    scenario_items = scenarios_payload.get("scenarios", [])
    if not isinstance(scenario_items, list):
        raise ValueError("scenarios file must contain array at key 'scenarios'")

    if args.generated_scenarios:
        generated_path = Path(args.generated_scenarios)
        if generated_path.exists():
            generated_payload = _read_json(generated_path)
            generated_items = generated_payload.get("scenarios", [])
            if isinstance(generated_items, list):
                scenario_items = list(scenario_items) + list(generated_items)

    scenario_items = sorted(
        [item for item in scenario_items if isinstance(item, dict)],
        key=lambda item: str(item.get("id", "")),
    )

    changed_files: List[str] = []
    if args.changed_files:
        changed_files = read_changed_files(args.changed_files)

    profile_cfg = policy.get("blockingProfile" if args.profile == "blocking" else "nightlyProfile", {})
    if not isinstance(profile_cfg, dict):
        profile_cfg = {}

    max_runtime_contract = int(policy.get("maxScenarioRuntimeSecs", args.max_scenario_runtime_secs) or args.max_scenario_runtime_secs)
    max_runtime = max(1, min(max_runtime_contract, args.max_scenario_runtime_secs))

    results: List[Dict[str, Any]] = []
    failure_classes: Dict[str, int] = {}

    for raw in scenario_items:
        if not _scenario_applies(raw, changed_files):
            continue

        scenario_timeout = int(raw.get("timeout_secs", max_runtime) or max_runtime)
        scenario_timeout = max(1, min(scenario_timeout, max_runtime))

        judged = evaluate_judge(
            scenario=raw,
            repo_root=repo_root,
            seed=args.seed,
            timeout_secs=scenario_timeout,
        )

        item = {
            "scenario_id": str(raw.get("id", "unknown")),
            "seed": args.seed,
            "surface": str(raw.get("surface", "unknown")),
            "failure_class": str(raw.get("failure_class", "unknown")),
            "pass": bool(judged.get("pass", False)),
            "duration_ms": int(judged.get("duration_ms", 0) or 0),
            "observed": str(judged.get("observed", "")),
            "expected": str(judged.get("expected", "")),
            "failure_reason": str(judged.get("failure_reason", "")),
            "artifacts": [str(x) for x in judged.get("artifacts", []) if str(x).strip()],
            "remediable": bool(raw.get("remediable", False)),
            "owner": str(raw.get("owner", "")),
            "severity": str(raw.get("severity", "medium")).lower(),
            "line_guess": int(judged.get("line_guess", 1) or 1),
        }
        item["deterministic_hash"] = _hash_result(item)

        if not item["pass"]:
            klass = item["failure_class"]
            failure_classes[klass] = failure_classes.get(klass, 0) + 1

        results.append(item)

    results.sort(key=lambda x: str(x.get("scenario_id", "")))

    total = len(results)
    passed = sum(1 for item in results if bool(item.get("pass")))
    failed = total - passed
    pass_rate = 1.0 if total == 0 else float(passed / total)

    blocking_threshold = float(policy.get("blockingThreshold", 1.0) or 1.0)
    all_blocking_passed = pass_rate >= blocking_threshold and (failed == 0 if blocking_threshold >= 0.9999 else True)

    summary = {
        "total": total,
        "passed": passed,
        "failed": failed,
        "pass_rate": pass_rate,
        "blocking_threshold": blocking_threshold,
        "all_blocking_passed": all_blocking_passed,
        "failure_classes": failure_classes,
    }

    eval_results = {
        "head_sha": args.head_sha,
        "profile": args.profile,
        "seed": args.seed,
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "summary": summary,
        "results": results,
        "profile_config": {
            "llm_mode": str(profile_cfg.get("llm_mode", "mock_frozen")),
            "network_mode": str(profile_cfg.get("network_mode", "isolated")),
            "seed": int(profile_cfg.get("seed", args.seed) or args.seed),
        },
    }

    max_findings = int(policy.get("maxRemediationFindingsPerRun", 25) or 25)
    failing_rows = [row for row in results if not bool(row.get("pass")) and bool(row.get("remediable"))]
    failing_rows.sort(
        key=lambda row: (
            _severity_rank(str(row.get("severity", "medium"))),
            str(row.get("scenario_id", "")),
        )
    )

    findings: List[Dict[str, Any]] = []
    scenario_lookup = {str(item.get("id", "")): item for item in scenario_items if isinstance(item, dict)}
    for row in failing_rows[:max_findings]:
        scenario = scenario_lookup.get(str(row.get("scenario_id", "")), {})
        if not isinstance(scenario, dict):
            scenario = {}
        finding = _normalize_finding(scenario, row)
        findings.append(finding)

    findings_payload = {
        "head_sha": args.head_sha,
        "provider": "eval",
        "status": "success",
        "summary": {
            "total_failed": failed,
            "actionable": len(findings),
        },
        "findings": findings,
        "errors": [],
    }

    _write_json(out_dir / "eval-results.json", eval_results)
    _write_json(out_dir / "eval-findings.json", findings_payload)
    _write_text(
        out_dir / "eval-summary.md",
        _build_summary_markdown(
            head_sha=args.head_sha,
            profile=args.profile,
            seed=args.seed,
            summary=summary,
            results=results,
        ),
    )

    enforce = _enforce_enabled(args.profile, args.enforce)
    exit_code = 1 if (enforce and not all_blocking_passed) else 0

    print(
        json.dumps(
            {
                "ok": exit_code == 0,
                "enforce": enforce,
                "all_blocking_passed": all_blocking_passed,
                "failed": failed,
                "out_dir": str(out_dir),
            },
            indent=2,
            sort_keys=True,
        )
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
