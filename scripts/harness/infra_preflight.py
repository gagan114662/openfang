#!/usr/bin/env python3
"""Infra/environment preflight for OpenFang CI workflows."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import socket
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Tuple


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run infra preflight checks for OpenFang workflows")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Policy contract path")
    parser.add_argument("--workflow", required=True, help="Workflow name to evaluate")
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="Repository in owner/repo form")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Token env var used for GitHub API checks")
    parser.add_argument("--require-env", action="append", default=[], help="Additional required env var (repeatable)")
    parser.add_argument("--timeout-secs", type=int, default=10, help="HTTP/socket timeout seconds")
    parser.add_argument(
        "--out",
        default="artifacts/infra-preflight-report.json",
        help="Output report JSON path",
    )
    parser.add_argument(
        "--attempts-override",
        type=int,
        default=0,
        help="Override retry attempts (0 uses policy value)",
    )
    return parser.parse_args()


def _read_json(path: Path) -> Dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"expected object in {path}")
    return payload


def _write_json(path: Path, payload: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _check_env(name: str) -> Tuple[str, str, bool]:
    value = os.getenv(name, "").strip()
    if value:
        return "pass", f"{name} is configured", False
    return "fail", f"missing required env var: {name}", False


def _check_tool(name: str) -> Tuple[str, str, bool]:
    if shutil.which(name):
        return "pass", f"tool '{name}' found in PATH", False
    return "fail", f"tool '{name}' missing from PATH", False


def _check_dns(host: str) -> Tuple[str, str, bool]:
    try:
        socket.gethostbyname(host)
        return "pass", f"DNS resolved for {host}", True
    except Exception as exc:
        return "fail", f"DNS resolution failed for {host}: {exc}", True


def _check_github_api(repo: str, token: str, timeout_secs: int) -> Tuple[str, str, bool]:
    if not repo or "/" not in repo:
        return "fail", f"invalid repo value '{repo}'", False
    if not token:
        return "fail", "missing GitHub token for API reachability check", False

    safe_repo = urllib.parse.quote(repo, safe="/")
    url = f"https://api.github.com/repos/{safe_repo}"
    req = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Authorization": f"Bearer {token}",
        },
        method="GET",
    )

    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            if 200 <= int(resp.status) < 300:
                return "pass", f"GitHub API reachable for repo {repo}", True
            return "fail", f"unexpected GitHub API status={resp.status}", True
    except urllib.error.HTTPError as exc:
        if exc.code in {401, 403, 404}:
            return "fail", f"GitHub API auth/access failed status={exc.code}", False
        if exc.code >= 500:
            return "fail", f"GitHub API server error status={exc.code}", True
        return "fail", f"GitHub API request failed status={exc.code}", True
    except urllib.error.URLError as exc:
        return "fail", f"GitHub API network error: {exc.reason}", True
    except Exception as exc:  # pragma: no cover
        return "fail", f"unexpected GitHub API error: {exc}", True


def _policy_settings(contract: Dict[str, Any]) -> Dict[str, Any]:
    eval_policy = contract.get("agentEvalPolicy", {})
    if not isinstance(eval_policy, dict):
        return {}
    preflight = eval_policy.get("infraPreflight", {})
    if not isinstance(preflight, dict):
        return {}
    return preflight


def _build_checks(args: argparse.Namespace) -> List[Tuple[str, str, str, bool]]:
    checks: List[Tuple[str, str, str, bool]] = []
    status, detail, transient = _check_env(args.token_env)
    checks.append(("token_env", status, detail, transient))

    for name in sorted({str(item).strip() for item in args.require_env if str(item).strip()}):
        status, detail, transient = _check_env(name)
        checks.append((f"required_env:{name}", status, detail, transient))

    for tool in ("python3", "git"):
        status, detail, transient = _check_tool(tool)
        checks.append((f"tool:{tool}", status, detail, transient))

    status, detail, transient = _check_dns("api.github.com")
    checks.append(("dns:api.github.com", status, detail, transient))

    token = os.getenv(args.token_env, "").strip()
    status, detail, transient = _check_github_api(args.repo, token, args.timeout_secs)
    checks.append(("github_api", status, detail, transient))

    return checks


def main() -> int:
    args = parse_args()
    contract = _read_json(Path(args.contract))
    preflight_cfg = _policy_settings(contract)

    enabled = bool(preflight_cfg.get("enabled", True))
    required_workflows = preflight_cfg.get("requiredForWorkflows", [])
    required_workflows = required_workflows if isinstance(required_workflows, list) else []
    required = args.workflow in {str(item) for item in required_workflows}

    retry_cfg = preflight_cfg.get("retryPolicy", {})
    retry_cfg = retry_cfg if isinstance(retry_cfg, dict) else {}
    attempts = int(retry_cfg.get("attempts", 3) or 3)
    if args.attempts_override > 0:
        attempts = args.attempts_override
    attempts = max(1, attempts)
    backoff = retry_cfg.get("backoffSeconds", [5, 20, 60])
    backoff = [int(v) for v in backoff if isinstance(v, int) and v >= 0]
    if not backoff:
        backoff = [5, 20, 60]

    report: Dict[str, Any] = {
        "workflow": args.workflow,
        "status": "missing",
        "required": required,
        "enabled": enabled,
        "attempts_configured": attempts,
        "attempts_used": 0,
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "transient_failures": 0,
        "checks": [],
        "errors": [],
    }

    if not enabled:
        report["status"] = "pass"
        report["errors"].append("infraPreflight.enabled is false")
        _write_json(Path(args.out), report)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0

    last_checks: List[Tuple[str, str, str, bool]] = []
    for attempt in range(1, attempts + 1):
        checks = _build_checks(args)
        last_checks = checks
        report["attempts_used"] = attempt

        failures = [item for item in checks if item[1] == "fail"]
        transient_failures = [item for item in failures if item[3]]
        report["transient_failures"] += len(transient_failures)

        if not failures:
            report["status"] = "pass"
            break

        if attempt >= attempts:
            report["status"] = "fail"
            break

        if transient_failures and len(transient_failures) == len(failures):
            sleep_for = backoff[min(attempt - 1, len(backoff) - 1)]
            time.sleep(max(0, sleep_for))
            continue

        report["status"] = "fail"
        break

    for name, status, detail, transient in last_checks:
        report["checks"].append(
            {
                "name": name,
                "status": status,
                "category": name.split(":", 1)[0],
                "detail": detail,
                "transient": bool(transient),
            }
        )
        if status == "fail":
            report["errors"].append(detail)

    if report["status"] == "missing":
        report["status"] = "error"
        report["errors"].append("preflight completed without terminal status")

    _write_json(Path(args.out), report)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
