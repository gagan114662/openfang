#!/usr/bin/env python3
"""Ingest trusted Claude PR feedback comments and normalize into findings."""

from __future__ import annotations

import argparse
import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple


DEFAULT_MARKER = "<!-- openfang-claude-feedback -->"
JSON_BLOCK_RE = re.compile(r"```(?:json)?\s*(\{.*?\})\s*```", re.IGNORECASE | re.DOTALL)
ACTIONABLE_KEYWORDS = ("fix", "error", "panic", "fail", "regression", "leak", "unsafe")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Ingest Claude feedback from PR comments")
    parser.add_argument("--repo", required=True, help="Repository in owner/repo format")
    parser.add_argument("--pr", type=int, required=True, help="Pull request number")
    parser.add_argument("--head-sha", required=True, help="Current PR head SHA")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Harness contract path")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Environment variable containing token")
    parser.add_argument("--out", default="artifacts/claude-findings.json", help="Output findings path")
    return parser.parse_args()


def _read_json(path: str) -> Dict[str, Any]:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _provider_cfg(contract: Dict[str, Any]) -> Dict[str, Any]:
    review_providers = contract.get("reviewProviders", {})
    if isinstance(review_providers, dict):
        providers = review_providers.get("providers", {})
        if isinstance(providers, dict):
            claude = providers.get("claude", {})
            if isinstance(claude, dict):
                return claude
    return {}


def _api_get(url: str, token: str) -> Dict[str, Any]:
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Authorization": f"Bearer {token}",
    }
    req = urllib.request.Request(url, headers=headers, method="GET")
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode("utf-8"))


def _paginate(url: str, token: str) -> List[Dict[str, Any]]:
    items: List[Dict[str, Any]] = []
    page = 1
    while True:
        sep = "&" if "?" in url else "?"
        payload = _api_get(f"{url}{sep}per_page=100&page={page}", token)
        if not isinstance(payload, list) or not payload:
            break
        page_items = [item for item in payload if isinstance(item, dict)]
        items.extend(page_items)
        if len(page_items) < 100:
            break
        page += 1
    return items


def _extract_author(entry: Dict[str, Any]) -> Tuple[str, Optional[int]]:
    user = entry.get("user", {})
    login = ""
    if isinstance(user, dict):
        login = str(user.get("login", "") or "")

    app_id: Optional[int] = None
    app = entry.get("performed_via_github_app", {})
    if isinstance(app, dict):
        raw = app.get("id")
        try:
            if raw is not None:
                app_id = int(raw)
        except Exception:
            app_id = None
    return login, app_id


def _extract_json_block(body: str, marker: str) -> Tuple[Optional[Dict[str, Any]], str]:
    if marker not in body:
        return None, "marker_missing"

    tail = body.split(marker, 1)[1]
    match = JSON_BLOCK_RE.search(tail)
    if not match:
        return None, "json_block_missing"

    raw = match.group(1)
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return None, f"invalid_json: {exc.msg}"

    if not isinstance(payload, dict):
        return None, "json_payload_not_object"

    return payload, "ok"


def _as_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on", "enabled"}
    return False


def _parse_int_csv(value: str) -> List[int]:
    out: List[int] = []
    for raw in value.split(","):
        token = raw.strip()
        if not token:
            continue
        try:
            out.append(int(token))
        except Exception:
            continue
    return out


def _is_actionable(finding: Dict[str, Any]) -> bool:
    if "actionable" in finding:
        return bool(finding.get("actionable"))

    severity = str(finding.get("severity", "")).strip().lower()
    summary = str(finding.get("summary", "")).strip().lower()
    confidence = float(finding.get("confidence", 0) or 0)

    if severity in {"critical", "high"}:
        return True
    if confidence >= 0.75 and severity in {"medium", "high", "critical"}:
        return True
    if any(keyword in summary for keyword in ACTIONABLE_KEYWORDS):
        return True
    return False


def _sha_matches(expected_sha: str, provided_sha: str) -> bool:
    expected = expected_sha.strip().lower()
    provided = provided_sha.strip().lower()
    if len(expected) < 7 or len(provided) < 7:
        return False
    return expected == provided or expected.startswith(provided) or provided.startswith(expected)


def _normalize_finding(finding: Dict[str, Any], idx: int) -> Tuple[Optional[Dict[str, Any]], str]:
    required = ["id", "severity", "confidence", "path", "line", "summary"]
    for key in required:
        if key not in finding:
            return None, f"finding[{idx}] missing required field: {key}"

    severity = str(finding.get("severity", "")).strip().lower()
    if severity not in {"critical", "high", "medium", "low", "info"}:
        return None, f"finding[{idx}] invalid severity: {severity}"

    try:
        confidence = float(finding.get("confidence"))
    except Exception:
        return None, f"finding[{idx}] confidence must be numeric"
    if confidence < 0 or confidence > 1:
        return None, f"finding[{idx}] confidence out of range 0..1"

    path = str(finding.get("path", "")).strip()
    if not path:
        return None, f"finding[{idx}] path cannot be empty"

    try:
        line = int(finding.get("line"))
    except Exception:
        return None, f"finding[{idx}] line must be integer"
    if line < 1:
        return None, f"finding[{idx}] line must be >= 1"

    summary = str(finding.get("summary", "")).strip()
    if not summary:
        return None, f"finding[{idx}] summary cannot be empty"

    normalized = {
        "id": str(finding.get("id", f"claude-{idx}")),
        "severity": severity,
        "confidence": confidence,
        "path": path,
        "line": line,
        "summary": summary,
        "actionable": _is_actionable(finding),
    }
    return normalized, "ok"


def _validate_payload(payload: Dict[str, Any], *, max_findings: int) -> Tuple[Optional[str], List[Dict[str, Any]], List[str]]:
    errors: List[str] = []

    head_sha = str(payload.get("head_sha", "")).strip()
    if len(head_sha) < 7:
        errors.append("head_sha must be a string with length >= 7")

    findings = payload.get("findings", [])
    if not isinstance(findings, list):
        errors.append("findings must be an array")
        findings = []

    normalized: List[Dict[str, Any]] = []
    for idx, finding in enumerate(findings[:max_findings], start=1):
        if not isinstance(finding, dict):
            errors.append(f"finding[{idx}] must be an object")
            continue
        item, detail = _normalize_finding(finding, idx)
        if item is None:
            errors.append(detail)
            continue
        normalized.append(item)

    if len(findings) > max_findings:
        errors.append(f"findings truncated to maxFindings={max_findings}")

    return head_sha if head_sha else None, normalized, errors


def _iter_comment_sources(repo: str, pr: int, token: str) -> Iterable[Dict[str, Any]]:
    safe_repo = urllib.parse.quote(repo, safe="/")

    issue_comments = _paginate(f"https://api.github.com/repos/{safe_repo}/issues/{pr}/comments", token)
    for item in issue_comments:
        login, app_id = _extract_author(item)
        yield {
            "source": "issue_comment",
            "id": str(item.get("id", "")),
            "body": str(item.get("body", "") or ""),
            "author_login": login,
            "author_app_id": app_id,
            "html_url": str(item.get("html_url", "") or ""),
        }

    review_comments = _paginate(f"https://api.github.com/repos/{safe_repo}/pulls/{pr}/comments", token)
    for item in review_comments:
        login, app_id = _extract_author(item)
        yield {
            "source": "review_comment",
            "id": str(item.get("id", "")),
            "body": str(item.get("body", "") or ""),
            "author_login": login,
            "author_app_id": app_id,
            "html_url": str(item.get("html_url", "") or ""),
        }

    reviews = _paginate(f"https://api.github.com/repos/{safe_repo}/pulls/{pr}/reviews", token)
    for item in reviews:
        login, app_id = _extract_author(item)
        yield {
            "source": "review_body",
            "id": str(item.get("id", "")),
            "body": str(item.get("body", "") or ""),
            "author_login": login,
            "author_app_id": app_id,
            "html_url": str(item.get("html_url", "") or ""),
        }


def main() -> int:
    args = parse_args()
    contract = _read_json(args.contract)
    provider_cfg = _provider_cfg(contract)

    github_cfg = provider_cfg.get("github", {}) if isinstance(provider_cfg.get("github"), dict) else {}
    parse_cfg = provider_cfg.get("parse", {}) if isinstance(provider_cfg.get("parse"), dict) else {}

    trusted_logins = {
        str(login).strip().lower()
        for login in github_cfg.get("trustedActorLogins", [])
        if str(login).strip()
    }
    trusted_app_ids = set()
    for app_id in github_cfg.get("trustedAppIds", []):
        try:
            trusted_app_ids.add(int(app_id))
        except Exception:
            continue
    trusted_app_ids_env = str(github_cfg.get("trustedAppIdsEnv", "") or "").strip()
    if trusted_app_ids_env:
        env_value = os.getenv(trusted_app_ids_env, "").strip()
        for app_id in _parse_int_csv(env_value):
            trusted_app_ids.add(app_id)

    marker = str(github_cfg.get("commentMarker", DEFAULT_MARKER) or DEFAULT_MARKER)
    require_head_sha_match = _as_bool(github_cfg.get("requireHeadShaMatch", True))
    max_findings = int(parse_cfg.get("maxFindings", 200) or 200)
    if max_findings < 1:
        max_findings = 1

    result: Dict[str, Any] = {
        "head_sha": args.head_sha,
        "provider": "claude",
        "status": "missing",
        "findings": [],
        "ingestion_metrics": {
            "trusted_comments_seen": 0,
            "parsed_comments": 0,
            "ignored_untrusted": 0,
            "ignored_stale": 0,
            "parse_errors": 0,
        },
        "trusted_sources": {
            "actor_logins": sorted(trusted_logins),
            "app_ids": sorted(trusted_app_ids),
            "app_ids_env": trusted_app_ids_env or None,
        },
        "errors": [],
    }

    token = os.getenv(args.token_env, "")
    if not token:
        result["errors"].append(f"missing token in env var: {args.token_env}")
        _write_json(args.out, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0

    if "/" not in args.repo:
        result["errors"].append("repo must be in owner/repo format")
        _write_json(args.out, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1

    findings: List[Dict[str, Any]] = []

    try:
        for comment in _iter_comment_sources(args.repo, args.pr, token):
            login = str(comment.get("author_login", "") or "").lower()
            app_id = comment.get("author_app_id")
            trusted = (login and login in trusted_logins) or (app_id is not None and int(app_id) in trusted_app_ids)
            if not trusted:
                result["ingestion_metrics"]["ignored_untrusted"] += 1
                continue

            result["ingestion_metrics"]["trusted_comments_seen"] += 1

            body = str(comment.get("body", "") or "")
            payload, parse_status = _extract_json_block(body, marker)
            if payload is None:
                if parse_status != "marker_missing":
                    result["ingestion_metrics"]["parse_errors"] += 1
                    result["errors"].append(
                        f"comment {comment.get('id', '')} ({comment.get('source', '')}): {parse_status}"
                    )
                continue

            parsed_head_sha, normalized_findings, errors = _validate_payload(payload, max_findings=max_findings)
            if errors:
                result["ingestion_metrics"]["parse_errors"] += 1
                result["errors"].append(
                    f"comment {comment.get('id', '')} ({comment.get('source', '')}): " + "; ".join(errors)
                )
                continue

            if require_head_sha_match and parsed_head_sha and not _sha_matches(args.head_sha, parsed_head_sha):
                result["ingestion_metrics"]["ignored_stale"] += 1
                continue

            result["ingestion_metrics"]["parsed_comments"] += 1
            findings.extend(normalized_findings)

    except urllib.error.HTTPError as exc:
        result["status"] = "error"
        result["errors"].append(f"GitHub API HTTP error: {exc.code}")
        _write_json(args.out, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    except urllib.error.URLError as exc:
        result["status"] = "error"
        result["errors"].append(f"GitHub API connection error: {exc.reason}")
        _write_json(args.out, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    except Exception as exc:  # pragma: no cover
        result["status"] = "error"
        result["errors"].append(f"unexpected ingestion error: {exc}")
        _write_json(args.out, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1

    # Deterministic order and max cap.
    findings = findings[:max_findings]
    findings.sort(key=lambda item: (str(item.get("path", "")), int(item.get("line", 1)), str(item.get("id", ""))))

    result["findings"] = findings
    parsed_comments = int(result["ingestion_metrics"].get("parsed_comments", 0) or 0)
    if parsed_comments > 0:
        result["status"] = "success"
    else:
        result["status"] = "missing"
        result["errors"].append("no trusted current-head Claude feedback payload found")

    _write_json(args.out, result)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
