#!/usr/bin/env python3
"""Constrained remediation runner for actionable review findings."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List


def _normalize(path: str) -> str:
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized.startswith("/"):
        normalized = normalized[1:]
    return normalized


def _match(path: str, pattern: str) -> bool:
    p = _normalize(path)
    pat = _normalize(pattern)
    return PurePosixPath(p).match(pat)


def _git_output(args: List[str]) -> str:
    proc = subprocess.run(args, check=True, capture_output=True, text=True)
    return proc.stdout.strip()


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _load_attempt_log(path: Path) -> Dict[str, int]:
    if not path.exists():
        return {}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(raw, dict):
            return {str(k): int(v) for k, v in raw.items()}
    except Exception:
        pass
    return {}


def _save_attempt_log(path: Path, data: Dict[str, int]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run constrained remediation for actionable findings")
    parser.add_argument("--findings", required=True, help="Path to review-findings.json")
    parser.add_argument("--head-sha", required=True, help="Current PR head SHA")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Harness contract path")
    parser.add_argument("--result-out", default="artifacts/remediation-result.json", help="Output remediation result path")
    parser.add_argument(
        "--apply-cmd",
        default="",
        help="Command that applies remediation changes (e.g. agent patch command)",
    )
    parser.add_argument(
        "--validation-cmd",
        action="append",
        default=[],
        help="Validation command to run after patching (repeatable)",
    )
    parser.add_argument(
        "--attempt-log",
        default=".harness/state/remediation-attempts.json",
        help="Local file tracking remediation attempts per head SHA",
    )
    parser.add_argument("--max-attempts", type=int, default=0, help="Override maximum attempts per SHA")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    findings_payload = json.loads(Path(args.findings).read_text(encoding="utf-8"))
    contract = json.loads(Path(args.contract).read_text(encoding="utf-8"))
    remediation_policy = contract.get("remediationPolicy", {})

    result: Dict[str, Any] = {
        "head_sha_in": args.head_sha,
        "head_sha_out": args.head_sha,
        "applied": False,
        "files_touched": [],
        "validation_passed": False,
        "errors": [],
    }

    provider = str(findings_payload.get("provider", "")).lower()
    supported_providers = {"", "greptile", "sentry"}
    if provider not in supported_providers:
        result["errors"].append(f"unsupported provider for remediation: {provider}")
        _write_json(args.result_out, result)
        return 1

    actionable_findings = [
        finding for finding in findings_payload.get("findings", []) if bool(finding.get("actionable", False))
    ]
    if not actionable_findings:
        result["validation_passed"] = True
        _write_json(args.result_out, result)
        return 0

    max_attempts = args.max_attempts or int(remediation_policy.get("maxAttemptsPerSha", 1))
    attempt_log_path = Path(args.attempt_log)
    attempt_log = _load_attempt_log(attempt_log_path)
    attempts_so_far = int(attempt_log.get(args.head_sha, 0))

    if attempts_so_far >= max_attempts:
        result["errors"].append(
            f"max remediation attempts exceeded for {args.head_sha}: {attempts_so_far}/{max_attempts}"
        )
        _write_json(args.result_out, result)
        return 1

    apply_cmd = args.apply_cmd.strip()
    if not apply_cmd:
        result["errors"].append("no remediation apply command configured")
        _write_json(args.result_out, result)
        return 1

    attempt_log[args.head_sha] = attempts_so_far + 1
    _save_attempt_log(attempt_log_path, attempt_log)

    before_sha = _git_output(["git", "rev-parse", "HEAD"])

    apply_proc = subprocess.run(apply_cmd, shell=True, text=True, capture_output=True)
    if apply_proc.returncode != 0:
        result["errors"].append(f"apply command failed: {apply_proc.stderr.strip() or apply_proc.stdout.strip()}")
        _write_json(args.result_out, result)
        return 1

    changed_raw = _git_output(["git", "diff", "--name-only"])
    files_touched = [line.strip() for line in changed_raw.splitlines() if line.strip()]
    result["files_touched"] = files_touched

    if not files_touched:
        result["errors"].append("remediation command made no file changes")
        _write_json(args.result_out, result)
        return 1

    allowed_globs = [str(item) for item in remediation_policy.get("allowedPathGlobs", [])]
    forbidden_globs = [str(item) for item in remediation_policy.get("forbiddenPathGlobs", [])]

    for touched in files_touched:
        normalized = _normalize(touched)
        if forbidden_globs and any(_match(normalized, pattern) for pattern in forbidden_globs):
            result["errors"].append(f"forbidden path modified by remediation: {normalized}")
        if allowed_globs and not any(_match(normalized, pattern) for pattern in allowed_globs):
            result["errors"].append(f"path outside allowed scope modified by remediation: {normalized}")

    if result["errors"]:
        _write_json(args.result_out, result)
        return 1

    validation_cmds = args.validation_cmd or [str(cmd) for cmd in remediation_policy.get("validationCommands", [])]
    for cmd in validation_cmds:
        proc = subprocess.run(cmd, shell=True, text=True, capture_output=True)
        if proc.returncode != 0:
            result["errors"].append(f"validation failed for '{cmd}': {proc.stderr.strip() or proc.stdout.strip()}")
            _write_json(args.result_out, result)
            return 1

    after_sha = _git_output(["git", "rev-parse", "HEAD"])
    result["head_sha_in"] = before_sha
    result["head_sha_out"] = after_sha
    result["applied"] = True
    result["validation_passed"] = True

    _write_json(args.result_out, result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
