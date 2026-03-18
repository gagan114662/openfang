#!/usr/bin/env python3
"""Fetch unresolved Sentry issues and normalize them into harness findings JSON."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List

from sentry_common import load_sentry_config, resolve_sentry_token, resolve_sentry_value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Collect actionable Sentry issues for remediation")
    parser.add_argument("--org", help="Sentry organization slug")
    parser.add_argument("--project", help="Sentry project slug")
    parser.add_argument("--base-url", default="", help="Sentry base URL")
    parser.add_argument("--token-env", default="SENTRY_AUTH_TOKEN", help="Env var with Sentry auth token")
    parser.add_argument("--query", default="is:unresolved level:error", help="Sentry issue query")
    parser.add_argument("--limit", type=int, default=20, help="Maximum issues to fetch")
    parser.add_argument("--out", default="artifacts/sentry-findings.json", help="Output findings JSON path")
    parser.add_argument("--config", default=str(Path.home() / ".openfang" / "config.toml"), help="OpenFang config path")
    parser.add_argument(
        "--allow-missing-token",
        action="store_true",
        help="Write an empty findings payload instead of failing if token is absent",
    )
    return parser.parse_args()


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _severity_from_level(level: str) -> str:
    mapping = {
        "fatal": "critical",
        "error": "high",
        "warning": "medium",
        "info": "low",
        "debug": "low",
    }
    return mapping.get(level.lower(), "medium")


def _confidence_from_severity(severity: str) -> float:
    if severity == "critical":
        return 0.98
    if severity == "high":
        return 0.9
    if severity == "medium":
        return 0.7
    return 0.5


def _coerce_line(metadata: Dict[str, Any]) -> int:
    for key in ("lineNo", "lineno", "line", "line_number"):
        value = metadata.get(key)
        if value is None:
            continue
        try:
            parsed = int(value)
            if parsed > 0:
                return parsed
        except (TypeError, ValueError):
            continue
    return 1


def _first_non_empty(values: List[str]) -> str:
    for value in values:
        stripped = value.strip()
        if stripped:
            return stripped
    return ""


def _git_head_sha() -> str:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
        return proc.stdout.strip()
    except Exception:
        return ""


def _fetch_sentry_issues(
    base_url: str,
    org: str,
    project: str,
    token: str,
    query: str,
    limit: int,
) -> List[Dict[str, Any]]:
    params = urllib.parse.urlencode({"query": query, "limit": str(limit)})
    endpoint = f"{base_url.rstrip('/')}/api/0/projects/{org}/{project}/issues/?{params}"
    req = urllib.request.Request(
        endpoint,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
        },
        method="GET",
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        payload = json.loads(response.read().decode("utf-8"))
        if isinstance(payload, list):
            return payload
        return []


def _normalize_issue(issue: Dict[str, Any], idx: int) -> Dict[str, Any]:
    metadata = issue.get("metadata") or {}
    level = str(issue.get("level", "error")).lower()
    severity = _severity_from_level(level)
    short_id = str(issue.get("shortId", "")).strip()
    title = str(issue.get("title", "")).strip() or str(metadata.get("title", "")).strip() or "Sentry issue"
    culprit = str(issue.get("culprit", "")).strip()
    summary = f"[{short_id}] {title}" if short_id else title
    if culprit:
        summary = f"{summary} ({culprit})"

    path = _first_non_empty(
        [
            str(metadata.get("filename", "")),
            str(metadata.get("abs_path", "")),
            str(metadata.get("path", "")),
        ]
    )

    status = str(issue.get("status", "unresolved")).lower()
    actionable = status != "resolved" and severity in {"critical", "high", "medium"}

    return {
        "id": str(issue.get("id", f"sentry-{idx}")),
        "severity": severity,
        "confidence": _confidence_from_severity(severity),
        "path": path,
        "line": _coerce_line(metadata),
        "summary": summary,
        "actionable": actionable,
        "source": {
            "short_id": short_id,
            "status": status,
            "permalink": str(issue.get("permalink", "")).strip(),
        },
    }


def main() -> int:
    args = parse_args()
    sentry_cfg = load_sentry_config(args.config)
    token = resolve_sentry_token(sentry_cfg, args.token_env)
    org = resolve_sentry_value(args.org, sentry_cfg, "org_slug", ["SENTRY_ORG_SLUG", "OPENFANG_SENTRY_ORG"])
    project = resolve_sentry_value(args.project, sentry_cfg, "project_slug", ["SENTRY_PROJECT_SLUG", "OPENFANG_SENTRY_PROJECT"])
    base_url = resolve_sentry_value(args.base_url, sentry_cfg, "api_base_url", ["SENTRY_BASE_URL", "OPENFANG_SENTRY_BASE_URL"], "https://sentry.io")

    payload: Dict[str, Any] = {
        "provider": "sentry",
        "status": "missing",
        "head_sha": _git_head_sha(),
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "org": org,
        "project": project,
        "query": args.query,
        "findings": [],
        "errors": [],
    }

    if not org or not project:
        payload["errors"].append("missing Sentry org/project in args, env, or config")
        _write_json(args.out, payload)
        return 1

    if not token:
        message = f"missing token in env var: {args.token_env}"
        payload["errors"].append(message)
        _write_json(args.out, payload)
        if args.allow_missing_token:
            return 0
        return 1

    try:
        issues = _fetch_sentry_issues(
            base_url=base_url,
            org=org,
            project=project,
            token=token,
            query=args.query,
            limit=args.limit,
        )
        payload["findings"] = [_normalize_issue(issue, idx) for idx, issue in enumerate(issues, start=1)]
        payload["status"] = "success"
        _write_json(args.out, payload)
        return 0
    except urllib.error.HTTPError as exc:
        payload["status"] = "error"
        payload["errors"].append(f"Sentry API HTTP error: {exc.code}")
        _write_json(args.out, payload)
        return 1
    except urllib.error.URLError as exc:
        payload["status"] = "error"
        payload["errors"].append(f"Sentry API connection error: {exc.reason}")
        _write_json(args.out, payload)
        return 1
    except Exception as exc:
        payload["status"] = "error"
        payload["errors"].append(f"Unexpected error: {exc}")
        _write_json(args.out, payload)
        return 1


if __name__ == "__main__":
    sys.exit(main())
