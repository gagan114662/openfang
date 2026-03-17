#!/usr/bin/env python3
"""Upsert sticky GitHub issue for infra preflight incidents."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Upsert infra incident issue from preflight report")
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="Repository in owner/repo format")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Env var name for GitHub token")
    parser.add_argument(
        "--report",
        default="artifacts/infra-preflight-report.json",
        help="Infra preflight report path",
    )
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Policy contract path")
    parser.add_argument("--title", default="OpenFang Infra Incident", help="Incident issue title")
    parser.add_argument("--out", default="artifacts/infra-incident-upsert.json", help="Output status JSON path")
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


def _api_request(
    *,
    method: str,
    url: str,
    token: str,
    payload: Optional[Dict[str, Any]] = None,
) -> Any:
    data = None
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Authorization": f"Bearer {token}",
    }
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read().decode("utf-8")
        return json.loads(body) if body else {}


def _incident_settings(contract: Dict[str, Any]) -> Dict[str, str]:
    eval_policy = contract.get("agentEvalPolicy", {})
    if not isinstance(eval_policy, dict):
        return {"enabled": "true", "label": "infra-incident", "marker": "<!-- openfang-infra-incident -->"}
    preflight = eval_policy.get("infraPreflight", {})
    if not isinstance(preflight, dict):
        return {"enabled": "true", "label": "infra-incident", "marker": "<!-- openfang-infra-incident -->"}
    incident = preflight.get("incident", {})
    if not isinstance(incident, dict):
        incident = {}
    return {
        "enabled": str(incident.get("enabled", True)).lower(),
        "label": str(incident.get("label", "infra-incident")),
        "marker": str(incident.get("marker", "<!-- openfang-infra-incident -->")),
    }


def _find_existing_issue(repo: str, token: str, marker: str, label: str) -> Optional[Dict[str, Any]]:
    safe_repo = urllib.parse.quote(repo, safe="/")
    safe_label = urllib.parse.quote(label, safe="")
    url = f"https://api.github.com/repos/{safe_repo}/issues?state=all&labels={safe_label}&per_page=100"
    payload = _api_request(method="GET", url=url, token=token)
    if not isinstance(payload, list):
        return None
    for item in payload:
        if not isinstance(item, dict):
            continue
        body = str(item.get("body", "") or "")
        if marker in body:
            return item
    return None


def _issue_body(marker: str, report: Dict[str, Any]) -> str:
    errors = report.get("errors", [])
    errors = [str(item) for item in errors] if isinstance(errors, list) else []
    checks = report.get("checks", [])
    checks = [item for item in checks if isinstance(item, dict)] if isinstance(checks, list) else []

    lines = [
        marker,
        "## OpenFang Infra Incident",
        "",
        f"- Workflow: `{report.get('workflow', 'unknown')}`",
        f"- Status: `{report.get('status', 'missing')}`",
        f"- Attempts used: `{report.get('attempts_used', 0)}` / `{report.get('attempts_configured', 0)}`",
        f"- Updated at: `{dt.datetime.now(tz=dt.timezone.utc).isoformat()}`",
        "",
        "### Failing Checks",
        "",
    ]
    failed = [item for item in checks if str(item.get("status", "")).lower() == "fail"]
    if failed:
        for item in failed:
            lines.append(f"- `{item.get('name', 'unknown')}`: {item.get('detail', '')}")
    else:
        lines.append("- none")

    if errors:
        lines.extend(["", "### Errors", ""])
        for err in errors:
            lines.append(f"- {err}")

    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    report = _read_json(Path(args.report), {"status": "missing", "workflow": "unknown", "errors": []})
    contract = _read_json(Path(args.contract), {})
    settings = _incident_settings(contract)

    out: Dict[str, Any] = {
        "status": "missing",
        "repo": args.repo,
        "issue_number": None,
        "action": "none",
        "errors": [],
    }

    enabled = settings.get("enabled", "true") in {"true", "1", "yes", "on"}
    label = settings.get("label", "infra-incident")
    marker = settings.get("marker", "<!-- openfang-infra-incident -->")
    token = os.getenv(args.token_env, "").strip()

    if not enabled:
        out["status"] = "success"
        out["action"] = "disabled"
        _write_json(Path(args.out), out)
        print(json.dumps(out, indent=2, sort_keys=True))
        return 0

    if not args.repo or "/" not in args.repo:
        out["status"] = "error"
        out["errors"].append(f"invalid repo value: {args.repo}")
        _write_json(Path(args.out), out)
        print(json.dumps(out, indent=2, sort_keys=True))
        return 1
    if not token:
        out["status"] = "error"
        out["errors"].append(f"missing token in env var: {args.token_env}")
        _write_json(Path(args.out), out)
        print(json.dumps(out, indent=2, sort_keys=True))
        return 1

    try:
        issue = _find_existing_issue(args.repo, token, marker, label)
        safe_repo = urllib.parse.quote(args.repo, safe="/")
        failing = str(report.get("status", "")).lower() != "pass"

        if failing:
            body = _issue_body(marker, report)
            if issue is None:
                payload = {"title": args.title, "body": body, "labels": [label]}
                try:
                    created = _api_request(
                        method="POST",
                        url=f"https://api.github.com/repos/{safe_repo}/issues",
                        token=token,
                        payload=payload,
                    )
                except urllib.error.HTTPError:
                    # fallback for missing label
                    payload = {"title": args.title, "body": body}
                    created = _api_request(
                        method="POST",
                        url=f"https://api.github.com/repos/{safe_repo}/issues",
                        token=token,
                        payload=payload,
                    )
                out["issue_number"] = int(created.get("number", 0) or 0)
                out["action"] = "created"
            else:
                issue_num = int(issue.get("number", 0) or 0)
                out["issue_number"] = issue_num
                state = str(issue.get("state", "")).lower()
                if state != "open":
                    _api_request(
                        method="PATCH",
                        url=f"https://api.github.com/repos/{safe_repo}/issues/{issue_num}",
                        token=token,
                        payload={"state": "open", "body": body, "title": args.title},
                    )
                    out["action"] = "reopened"
                else:
                    _api_request(
                        method="PATCH",
                        url=f"https://api.github.com/repos/{safe_repo}/issues/{issue_num}",
                        token=token,
                        payload={"body": body, "title": args.title},
                    )
                    out["action"] = "updated"
                _api_request(
                    method="POST",
                    url=f"https://api.github.com/repos/{safe_repo}/issues/{issue_num}/comments",
                    token=token,
                    payload={
                        "body": f"{marker}\nInfra preflight still failing for `{report.get('workflow', 'unknown')}` at `{dt.datetime.now(tz=dt.timezone.utc).isoformat()}`."
                    },
                )
        else:
            if issue is not None:
                issue_num = int(issue.get("number", 0) or 0)
                out["issue_number"] = issue_num
                _api_request(
                    method="POST",
                    url=f"https://api.github.com/repos/{safe_repo}/issues/{issue_num}/comments",
                    token=token,
                    payload={
                        "body": f"{marker}\nInfra preflight recovered for `{report.get('workflow', 'unknown')}` at `{dt.datetime.now(tz=dt.timezone.utc).isoformat()}`. Closing incident."
                    },
                )
                _api_request(
                    method="PATCH",
                    url=f"https://api.github.com/repos/{safe_repo}/issues/{issue_num}",
                    token=token,
                    payload={"state": "closed"},
                )
                out["action"] = "closed"
            else:
                out["action"] = "no_open_incident"

        out["status"] = "success"
        _write_json(Path(args.out), out)
        print(json.dumps(out, indent=2, sort_keys=True))
        return 0
    except urllib.error.HTTPError as exc:
        out["status"] = "error"
        out["errors"].append(f"GitHub API HTTP error: {exc.code}")
    except urllib.error.URLError as exc:
        out["status"] = "error"
        out["errors"].append(f"GitHub API network error: {exc.reason}")
    except Exception as exc:  # pragma: no cover
        out["status"] = "error"
        out["errors"].append(f"unexpected error: {exc}")

    _write_json(Path(args.out), out)
    print(json.dumps(out, indent=2, sort_keys=True))
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
