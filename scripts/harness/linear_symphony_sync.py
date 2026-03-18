#!/usr/bin/env python3
"""Sync actionable Sentry/GitHub events into the Symphony Linear project."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional


LINEAR_API = "https://api.linear.app/graphql"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Sync actionable events to Linear Symphony")
    parser.add_argument("--mode", choices=("sentry-findings", "github-remediation"), required=True)
    parser.add_argument("--team-key", default=os.getenv("LINEAR_TEAM_KEY", "GET"))
    parser.add_argument("--project-name", default=os.getenv("LINEAR_PROJECT_NAME", "Symphony"))
    parser.add_argument("--linear-token-env", default="LINEAR_API_KEY")
    parser.add_argument("--findings", default="", help="Findings JSON payload")
    parser.add_argument("--remediation-result", default="", help="Remediation result JSON payload")
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", ""), help="owner/repo")
    parser.add_argument("--branch", default=os.getenv("GITHUB_REF_NAME", ""), help="branch name")
    parser.add_argument("--pr-url", default=os.getenv("LINEAR_SYNC_PR_URL", ""), help="PR URL when available")
    parser.add_argument("--head-sha", default=os.getenv("GITHUB_SHA", ""), help="Current head SHA")
    parser.add_argument("--state", default="", help="Workflow status name override")
    parser.add_argument("--out", default="artifacts/linear-symphony-sync.json")
    return parser.parse_args()


def _read_json(path: str) -> Dict[str, Any]:
    target = Path(path)
    if not path or not target.exists():
        return {}
    payload = json.loads(target.read_text(encoding="utf-8"))
    return payload if isinstance(payload, dict) else {}


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def daemon_base_url() -> str:
    explicit = os.environ.get("OPENFANG_API_BASE", "").strip()
    if explicit:
        return explicit.rstrip("/")

    daemon_info = Path.home() / ".openfang" / "daemon.json"
    if daemon_info.exists():
        try:
            data = json.loads(daemon_info.read_text(encoding="utf-8"))
            listen_addr = str(data.get("listen_addr") or "").strip()
            if listen_addr.startswith(("http://", "https://")):
                return listen_addr.rstrip("/")
            if listen_addr:
                return f"http://{listen_addr}"
        except Exception:
            return ""
    return ""


def emit_structured_event(event_kind: str, level: str, attributes: Dict[str, Any]) -> None:
    base_url = daemon_base_url()
    if not base_url:
        return
    payload = {
        "body": event_kind,
        "level": level,
        "attributes": {
            "event.kind": event_kind,
            **attributes,
        },
    }
    req = urllib.request.Request(
        f"{base_url}/api/telemetry/structured",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=5):
            return
    except (urllib.error.URLError, TimeoutError):
        return


def gql(token: str, query: str, variables: Dict[str, Any]) -> Dict[str, Any]:
    req = urllib.request.Request(
        LINEAR_API,
        data=json.dumps({"query": query, "variables": variables}).encode("utf-8"),
        headers={
            "Authorization": token,
            "Content-Type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        payload = json.loads(response.read().decode("utf-8"))
    if payload.get("errors"):
        raise RuntimeError(json.dumps(payload["errors"]))
    data = payload.get("data")
    return data if isinstance(data, dict) else {}


def team_project_context(token: str, team_key: str, project_name: str, state_name: str) -> Dict[str, str]:
    query = """
    query SymphonyContext($teamKey: String!, $projectName: String!, $stateName: String!) {
      teams(filter: { key: { eq: $teamKey } }) {
        nodes { id key name }
      }
      projects(filter: { name: { eq: $projectName } }) {
        nodes { id name url }
      }
      workflowStates(filter: { name: { eq: $stateName }, team: { key: { eq: $teamKey } } }) {
        nodes { id name type }
      }
    }
    """
    data = gql(token, query, {"teamKey": team_key, "projectName": project_name, "stateName": state_name})
    teams = (((data.get("teams") or {}).get("nodes")) or [])
    projects = (((data.get("projects") or {}).get("nodes")) or [])
    states = (((data.get("workflowStates") or {}).get("nodes")) or [])
    if not teams:
        raise RuntimeError(f"Linear team not found: {team_key}")
    if not projects:
        raise RuntimeError(f"Linear project not found: {project_name}")
    if not states:
        raise RuntimeError(f"Linear workflow state not found: {state_name}")
    return {
        "team_id": teams[0]["id"],
        "project_id": projects[0]["id"],
        "project_url": projects[0].get("url", ""),
        "state_id": states[0]["id"],
    }


def find_issue_by_key(token: str, team_key: str, key: str) -> Optional[Dict[str, Any]]:
    query = """
    query ExistingSymphonyIssue($teamKey: String!, $key: String!) {
      issues(
        first: 1,
        filter: {
          team: { key: { eq: $teamKey } }
          title: { containsIgnoreCase: $key }
        }
      ) {
        nodes {
          id
          identifier
          title
          url
          description
          state { name type }
        }
      }
    }
    """
    data = gql(token, query, {"teamKey": team_key, "key": key})
    nodes = (((data.get("issues") or {}).get("nodes")) or [])
    return nodes[0] if nodes else None


def create_issue(
    token: str,
    *,
    team_id: str,
    project_id: str,
    state_id: str,
    title: str,
    description: str,
    priority: int,
) -> Dict[str, Any]:
    mutation = """
    mutation CreateSymphonyIssue($input: IssueCreateInput!) {
      issueCreate(input: $input) {
        success
        issue { id identifier title url }
      }
    }
    """
    data = gql(
        token,
        mutation,
        {
            "input": {
                "teamId": team_id,
                "projectId": project_id,
                "stateId": state_id,
                "title": title,
                "description": description,
                "priority": priority,
            }
        },
    )
    return ((data.get("issueCreate") or {}).get("issue")) or {}


def update_issue(token: str, issue_id: str, *, state_id: str, description: str, project_id: str) -> Dict[str, Any]:
    mutation = """
    mutation UpdateSymphonyIssue($id: String!, $input: IssueUpdateInput!) {
      issueUpdate(id: $id, input: $input) {
        success
        issue { id identifier title url }
      }
    }
    """
    data = gql(
        token,
        mutation,
        {
            "id": issue_id,
            "input": {
                "stateId": state_id,
                "description": description,
                "projectId": project_id,
            },
        },
    )
    return ((data.get("issueUpdate") or {}).get("issue")) or {}


def create_comment(token: str, issue_id: str, body: str) -> None:
    mutation = """
    mutation AddSymphonyComment($input: CommentCreateInput!) {
      commentCreate(input: $input) {
        success
        comment { id }
      }
    }
    """
    gql(token, mutation, {"input": {"issueId": issue_id, "body": body}})


def severity_priority(severity: str) -> int:
    mapping = {"critical": 1, "high": 2, "medium": 3, "low": 4}
    return mapping.get(severity.lower(), 3)


def symphony_key_for_finding(payload: Dict[str, Any], finding: Dict[str, Any]) -> str:
    org = str(payload.get("org") or "unknown-org")
    project = str(payload.get("project") or "unknown-project")
    raw_id = str(finding.get("id") or "unknown")
    return f"sentry:{org}:{project}:{raw_id}"


def issue_description(
    *,
    key: str,
    summary: str,
    payload: Dict[str, Any],
    finding: Dict[str, Any],
    repo: str,
    branch: str,
    head_sha: str,
    pr_url: str,
) -> str:
    lines = [
        f"Symphony-Key: {key}",
        "",
        summary,
        "",
        f"- Source: {payload.get('provider', 'sentry')}",
        f"- Severity: {finding.get('severity', 'medium')}",
        f"- Confidence: {finding.get('confidence', '')}",
        f"- Org/Project: {payload.get('org', '')} / {payload.get('project', '')}",
        f"- Head SHA: {head_sha}",
        f"- Repo: {repo}",
        f"- Branch: {branch}",
    ]
    source = finding.get("source") or {}
    permalink = str(source.get("permalink") or "").strip()
    if permalink:
        lines.append(f"- Sentry permalink: {permalink}")
    path = str(finding.get("path") or "").strip()
    if path:
        lines.append(f"- Code path: {path}:{finding.get('line', 1)}")
    if pr_url:
        lines.append(f"- Remediation PR: {pr_url}")
    return "\n".join(lines).strip() + "\n"


def iter_actionable_findings(payload: Dict[str, Any]) -> Iterable[Dict[str, Any]]:
    findings = payload.get("findings")
    if not isinstance(findings, list):
        return []
    return [item for item in findings if isinstance(item, dict) and bool(item.get("actionable"))]


def sync_sentry_findings(token: str, args: argparse.Namespace, payload: Dict[str, Any]) -> List[Dict[str, Any]]:
    state_name = args.state or "Todo"
    context = team_project_context(token, args.team_key, args.project_name, state_name)
    synced: List[Dict[str, Any]] = []

    for finding in iter_actionable_findings(payload):
        key = symphony_key_for_finding(payload, finding)
        summary = str(finding.get("summary") or "Sentry finding")
        title = f"[{key}] {summary[:140]}"
        description = issue_description(
            key=key,
            summary=summary,
            payload=payload,
            finding=finding,
            repo=args.repo,
            branch=args.branch,
            head_sha=args.head_sha,
            pr_url=args.pr_url,
        )
        existing = find_issue_by_key(token, args.team_key, key)
        if existing:
            issue = update_issue(
                token,
                existing["id"],
                state_id=context["state_id"],
                description=description,
                project_id=context["project_id"],
            )
            create_comment(
                token,
                existing["id"],
                f"Updated from Sentry sync at {dt.datetime.now(dt.timezone.utc).isoformat()}.\n\n- Branch: {args.branch}\n- Head SHA: {args.head_sha}",
            )
            synced.append({"action": "updated", "key": key, "issue": issue or existing})
            continue

        issue = create_issue(
            token,
            team_id=context["team_id"],
            project_id=context["project_id"],
            state_id=context["state_id"],
            title=title,
            description=description,
            priority=severity_priority(str(finding.get("severity") or "medium")),
        )
        synced.append({"action": "created", "key": key, "issue": issue})

    return synced


def sync_github_remediation(token: str, args: argparse.Namespace, findings_payload: Dict[str, Any], remediation_payload: Dict[str, Any]) -> List[Dict[str, Any]]:
    state_name = args.state or ("In Review" if args.pr_url else "In Progress")
    if remediation_payload.get("validation_passed") and args.pr_url:
        state_name = "In Review"
    context = team_project_context(token, args.team_key, args.project_name, state_name)
    synced: List[Dict[str, Any]] = []
    comment_lines = [
        f"GitHub remediation update at {dt.datetime.now(dt.timezone.utc).isoformat()}",
        f"- Repo: {args.repo}",
        f"- Branch: {args.branch}",
        f"- Head SHA: {args.head_sha}",
        f"- PR URL: {args.pr_url or 'n/a'}",
        f"- Validation passed: {bool(remediation_payload.get('validation_passed'))}",
        f"- Applied: {bool(remediation_payload.get('applied'))}",
    ]
    if remediation_payload.get("errors"):
        comment_lines.append(f"- Errors: {remediation_payload.get('errors')}")
    comment = "\n".join(comment_lines)

    for finding in iter_actionable_findings(findings_payload):
        key = symphony_key_for_finding(findings_payload, finding)
        existing = find_issue_by_key(token, args.team_key, key)
        if not existing:
            continue
        description = issue_description(
            key=key,
            summary=str(finding.get("summary") or "Sentry finding"),
            payload=findings_payload,
            finding=finding,
            repo=args.repo,
            branch=args.branch,
            head_sha=args.head_sha,
            pr_url=args.pr_url,
        )
        issue = update_issue(
            token,
            existing["id"],
            state_id=context["state_id"],
            description=description,
            project_id=context["project_id"],
        )
        create_comment(token, existing["id"], comment)
        synced.append({"action": "updated", "key": key, "issue": issue or existing})
    return synced


def main() -> int:
    args = parse_args()
    token = os.getenv(args.linear_token_env, "").strip()
    findings_payload = _read_json(args.findings)
    remediation_payload = _read_json(args.remediation_result)

    result: Dict[str, Any] = {
        "mode": args.mode,
        "team_key": args.team_key,
        "project_name": args.project_name,
        "repo": args.repo,
        "branch": args.branch,
        "head_sha": args.head_sha,
        "pr_url": args.pr_url,
        "synced": [],
        "status": "missing",
        "errors": [],
    }

    if not token:
        result["errors"].append(f"missing Linear token in env var: {args.linear_token_env}")
        _write_json(args.out, result)
        return 0

    try:
        if args.mode == "sentry-findings":
            result["synced"] = sync_sentry_findings(token, args, findings_payload)
        else:
            result["synced"] = sync_github_remediation(token, args, findings_payload, remediation_payload)
        result["status"] = "success"
        _write_json(args.out, result)
        emit_structured_event(
            "linear.symphony.sync.completed",
            "info",
            {
                "linear.mode": args.mode,
                "linear.team_key": args.team_key,
                "linear.project_name": args.project_name,
                "linear.synced_count": len(result["synced"]),
                "github.repo": args.repo,
                "git.branch": args.branch,
                "git.head_sha": args.head_sha,
            },
        )
        return 0
    except urllib.error.HTTPError as exc:
        result["status"] = "error"
        result["errors"].append(f"Linear API HTTP error: {exc.code}")
    except urllib.error.URLError as exc:
        result["status"] = "error"
        result["errors"].append(f"Linear API connection error: {exc.reason}")
    except Exception as exc:
        result["status"] = "error"
        result["errors"].append(str(exc))

    _write_json(args.out, result)
    emit_structured_event(
        "linear.symphony.sync.failed",
        "warn",
        {
            "linear.mode": args.mode,
            "linear.team_key": args.team_key,
            "linear.project_name": args.project_name,
            "github.repo": args.repo,
            "git.branch": args.branch,
            "git.head_sha": args.head_sha,
            "error.count": len(result["errors"]),
        },
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
