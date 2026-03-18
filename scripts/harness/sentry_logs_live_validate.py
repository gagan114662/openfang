#!/usr/bin/env python3
"""Validate live Sentry unresolved errors for PR gating."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List

from sentry_common import load_sentry_config, resolve_sentry_token, resolve_sentry_value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate live Sentry unresolved issues")
    parser.add_argument("--org", help="Sentry organization slug")
    parser.add_argument("--project", help="Sentry project slug")
    parser.add_argument("--head-sha", required=True, help="Current PR head SHA")
    parser.add_argument("--base-url", default="", help="Sentry base URL")
    parser.add_argument("--token-env", default="SENTRY_AUTH_TOKEN", help="Env var with Sentry auth token")
    parser.add_argument("--query", default="is:unresolved level:error", help="Sentry issue query")
    parser.add_argument("--limit", type=int, default=20, help="Maximum issue count to inspect")
    parser.add_argument("--out", default="artifacts/sentry-logs-validation.json", help="Output JSON report")
    parser.add_argument("--config", default=str(Path.home() / ".openfang" / "config.toml"), help="OpenFang config path")
    return parser.parse_args()


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _fetch_issues(base_url: str, org: str, project: str, token: str, query: str, limit: int) -> List[Dict[str, Any]]:
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
        return payload if isinstance(payload, list) else []


def main() -> int:
    args = parse_args()
    sentry_cfg = load_sentry_config(args.config)
    token = resolve_sentry_token(sentry_cfg, args.token_env)
    org = resolve_sentry_value(args.org, sentry_cfg, "org_slug", ["SENTRY_ORG_SLUG", "OPENFANG_SENTRY_ORG"])
    project = resolve_sentry_value(args.project, sentry_cfg, "project_slug", ["SENTRY_PROJECT_SLUG", "OPENFANG_SENTRY_PROJECT"])
    base_url = resolve_sentry_value(args.base_url, sentry_cfg, "api_base_url", ["SENTRY_BASE_URL", "OPENFANG_SENTRY_BASE_URL"], "https://sentry.io")

    report: Dict[str, Any] = {
        "head_sha": args.head_sha,
        "checked_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "org": org,
        "project": project,
        "query": args.query,
        "status": "missing",
        "ok": False,
        "issue_count": 0,
        "issues": [],
        "errors": [],
    }

    if not org or not project:
        report["status"] = "missing"
        report["errors"].append("missing Sentry org/project in args, env, or config")
        _write_json(args.out, report)
        return 2

    if not token:
        report["status"] = "missing"
        report["errors"].append(f"missing token in env var: {args.token_env}")
        _write_json(args.out, report)
        return 2

    try:
        issues = _fetch_issues(
            base_url=base_url,
            org=org,
            project=project,
            token=token,
            query=args.query,
            limit=args.limit,
        )
        report["issues"] = [
            {
                "id": str(issue.get("id", "")),
                "shortId": str(issue.get("shortId", "")),
                "title": str(issue.get("title", "")),
                "status": str(issue.get("status", "")),
                "level": str(issue.get("level", "")),
                "permalink": str(issue.get("permalink", "")),
            }
            for issue in issues
        ]
        report["issue_count"] = len(report["issues"])
        report["ok"] = len(report["issues"]) == 0
        report["status"] = "pass" if report["ok"] else "fail"
        _write_json(args.out, report)
        return 0 if report["ok"] else 1
    except urllib.error.HTTPError as exc:
        report["status"] = "error"
        report["errors"].append(f"Sentry API HTTP error: {exc.code}")
    except urllib.error.URLError as exc:
        report["status"] = "error"
        report["errors"].append(f"Sentry API connection error: {exc.reason}")
    except Exception as exc:
        report["status"] = "error"
        report["errors"].append(f"Unexpected error: {exc}")

    _write_json(args.out, report)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
