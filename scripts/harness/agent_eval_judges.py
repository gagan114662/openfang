#!/usr/bin/env python3
"""Deterministic scenario judges for OpenFang eval runner."""

from __future__ import annotations

import json
import re
import subprocess
import time
from pathlib import Path
from typing import Any, Dict, Tuple

from agent_eval_fixtures import fixture_contains


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _json_get(payload: Any, field_path: str) -> Any:
    current = payload
    for segment in field_path.split("."):
        if isinstance(current, dict) and segment in current:
            current = current[segment]
            continue
        return None
    return current


def _find_line_for_pattern(path: Path, pattern: str) -> int:
    if not path.exists():
        return 1
    rx = re.compile(pattern)
    for idx, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if rx.search(line):
            return idx
    return 1


def evaluate_judge(
    *,
    scenario: Dict[str, Any],
    repo_root: Path,
    seed: int,
    timeout_secs: int,
) -> Dict[str, Any]:
    judge = scenario.get("judge", {})
    if not isinstance(judge, dict):
        judge = {}

    kind = str(judge.get("kind", "")).strip().lower()
    started = time.perf_counter()

    expected = scenario.get("expected", {})
    expected_text = json.dumps(expected, sort_keys=True)

    observed_text = ""
    failure_reason = ""
    artifacts = []
    passed = False

    try:
        if kind == "file_exists":
            target = repo_root / str(judge.get("path", ""))
            observed_text = f"exists={target.exists()} path={target}"
            passed = target.exists() and target.is_file()
            if not passed:
                failure_reason = "required file missing"

        elif kind in {"regex_present", "regex_absent"}:
            target = repo_root / str(judge.get("path", ""))
            pattern = str(judge.get("pattern", ""))
            if not pattern:
                raise ValueError("missing regex pattern")
            if not target.exists():
                observed_text = f"path missing: {target}"
                failure_reason = "target file missing"
                passed = False
            else:
                text = _read_text(target)
                hit = re.search(pattern, text, flags=re.MULTILINE) is not None
                observed_text = f"pattern_hit={hit} kind={kind}"
                passed = hit if kind == "regex_present" else not hit
                if not passed:
                    failure_reason = "regex expectation not met"

        elif kind == "json_field_exists":
            target = repo_root / str(judge.get("path", ""))
            field = str(judge.get("field", ""))
            payload = _read_json(target)
            value = _json_get(payload, field)
            observed_text = f"field={field} exists={value is not None}"
            passed = value is not None
            if not passed:
                failure_reason = f"json field missing: {field}"

        elif kind == "json_array_min_length":
            target = repo_root / str(judge.get("path", ""))
            field = str(judge.get("field", ""))
            min_length = int(judge.get("min_length", 1) or 1)
            payload = _read_json(target)
            value = _json_get(payload, field)
            actual = len(value) if isinstance(value, list) else -1
            observed_text = f"field={field} length={actual}"
            passed = isinstance(value, list) and actual >= min_length
            if not passed:
                failure_reason = f"json array {field} length {actual} < {min_length}"

        elif kind == "json_number_range":
            target = repo_root / str(judge.get("path", ""))
            field = str(judge.get("field", ""))
            min_value = float(judge.get("min", 0))
            max_value = float(judge.get("max", 1e9))
            payload = _read_json(target)
            value = _json_get(payload, field)
            numeric = float(value) if isinstance(value, (int, float)) else None
            observed_text = f"field={field} value={numeric}"
            passed = numeric is not None and min_value <= numeric <= max_value
            if not passed:
                failure_reason = f"json numeric field {field} outside range"

        elif kind == "command_exit":
            command = str(judge.get("command", "")).strip()
            expected_exit = int(judge.get("expected_exit", 0))
            if not command:
                raise ValueError("missing command")
            proc = subprocess.run(
                command,
                shell=True,
                cwd=str(repo_root),
                capture_output=True,
                text=True,
                timeout=max(1, timeout_secs),
            )
            observed_text = f"exit={proc.returncode} stdout={proc.stdout[:120].strip()} stderr={proc.stderr[:120].strip()}"
            passed = proc.returncode == expected_exit
            if not passed:
                failure_reason = f"command exit {proc.returncode} != {expected_exit}"

        elif kind == "fixture_contains":
            fixture_name = str(judge.get("fixture", "")).strip()
            needle = str(judge.get("contains", "")).strip()
            ok = fixture_contains(fixture_name, seed, needle)
            observed_text = f"fixture={fixture_name} contains={ok} needle={needle}"
            passed = ok
            if not passed:
                failure_reason = "frozen fixture lookup failed"

        else:
            observed_text = f"unsupported judge kind: {kind}"
            failure_reason = "unsupported judge kind"
            passed = False

    except subprocess.TimeoutExpired:
        observed_text = f"judge timed out after {timeout_secs}s"
        failure_reason = "scenario timeout"
        passed = False
    except Exception as exc:
        observed_text = f"judge error: {exc}"
        failure_reason = "judge execution error"
        passed = False

    duration_ms = int((time.perf_counter() - started) * 1000)

    finding_path = str(scenario.get("finding", {}).get("path", "") or "")
    hint_pattern = str(scenario.get("finding", {}).get("line_hint_pattern", "") or "")
    if finding_path and hint_pattern:
        artifacts.append(finding_path)
        line_guess = _find_line_for_pattern(repo_root / finding_path, hint_pattern)
    else:
        line_guess = int(scenario.get("finding", {}).get("line", 1) or 1)

    return {
        "pass": passed,
        "observed": observed_text,
        "expected": expected_text,
        "failure_reason": "" if passed else (failure_reason or "assertion failed"),
        "duration_ms": duration_ms,
        "artifacts": artifacts,
        "line_guess": line_guess,
    }
