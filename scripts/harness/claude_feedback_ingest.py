#!/usr/bin/env python3
"""Ingest Claude review findings from trusted GitHub PR comments/reviews."""

from __future__ import annotations

import argparse
import json
import os
import re
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple

from greptile_state import _is_actionable_from_heuristic


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Ingest Claude findings from PR comments")
    parser.add_argument("--repo", required=True, help="owner/repo")
    parser.add_argument("--pr", type=int, required=True, help="Pull request number")
    parser.add_argument("--head-sha", required=True, help="Current PR head SHA")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Harness contract path")
    parser.add_argument("--token-env", default="GITHUB_TOKEN", help="Environment variable storing GitHub token")
    parser.add_argument("--out", default="artifacts/claude-findings.json", help="Output findings JSON path")
    return parser.parse_args()


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _api_get_json(url: str, token: str) -> List[Dict[str, Any]]:
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Authorization": f"Bearer {token}",
    }
    req = urllib.request.Request(url, headers=headers, method="GET")
    with urllib.request.urlopen(req, timeout=30) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
        return payload if isinstance(payload, list) else []


def _parse_int_csv(value: str) -> List[int]:
    parsed: List[int] = []
    for chunk in (value or "").split(","):
        token = chunk.strip()
        if not token:
            continue
        try:
            parsed.append(int(token))
        except ValueError:
            continue
    return parsed


def _extract_payload_after_marker(body: str, marker: str) -> Optional[Dict[str, Any]]:
    if marker not in body:
        return None

    tail = body.split(marker, 1)[1].strip()

    fenced = re.search(r"```(?:json)?\s*(\{[\s\S]*?\})\s*```", tail, flags=re.IGNORECASE)
    if fenced:
        try:
            payload = json.loads(fenced.group(1))
            return payload if isinstance(payload, dict) else None
        except json.JSONDecodeError:
            return None

    first = tail.find("{")
    if first < 0:
        return None

    decoder = json.JSONDecoder()
    try:
        payload, _ = decoder.raw_decode(tail[first:])
        return payload if isinstance(payload, dict) else None
    except json.JSONDecodeError:
        return None


def _is_trusted_source(
    source: Dict[str, Any],
    *,
    trusted_app_ids: List[int],
    trusted_logins: List[str],
    maintainer_allowlist: List[str],
) -> bool:
    login = str((source.get("user") or {}).get("login", "")).strip().lower()
    app_id = (source.get("performed_via_github_app") or {}).get("id")

    if login and login in {item.lower() for item in maintainer_allowlist}:
        return True

    if trusted_app_ids:
        try:
            if int(app_id) in trusted_app_ids:
                return True
        except (TypeError, ValueError):
            return False
        return False

    if login and login in {item.lower() for item in trusted_logins}:
        return True
    return False


def _normalize_finding(
    finding: Dict[str, Any],
    *,
    weak_confidence_threshold: float,
    actionable_keywords: List[str],
    index: int,
) -> Dict[str, Any]:
    severity = str(finding.get("severity", "medium")).lower()
    confidence = float(finding.get("confidence", 0.0) or 0.0)
    summary = str(finding.get("summary", "")).strip()

    return {
        "id": str(finding.get("id", f"claude-{index}")),
        "severity": severity,
        "confidence": confidence,
        "path": str(finding.get("path", "")),
        "line": int(finding.get("line", 1) or 1),
        "summary": summary,
        "actionable": bool(
            finding.get(
                "actionable",
                _is_actionable_from_heuristic(
                    finding,
                    weak_confidence_threshold=weak_confidence_threshold,
                    actionable_keywords=actionable_keywords,
                ),
            )
        ),
    }


def _iter_comment_sources(repo: str, pr_number: int, token: str) -> Iterable[Dict[str, Any]]:
    issue_comments = _api_get_json(
        f"https://api.github.com/repos/{repo}/issues/{pr_number}/comments?per_page=100",
        token,
    )
    for comment in issue_comments:
        item = dict(comment)
        item["_source_type"] = "issue_comment"
        yield item

    review_comments = _api_get_json(
        f"https://api.github.com/repos/{repo}/pulls/{pr_number}/reviews?per_page=100",
        token,
    )
    for review in review_comments:
        item = dict(review)
        item["_source_type"] = "pull_review"
        yield item


def main() -> int:
    args = parse_args()
    token = os.getenv(args.token_env, "").strip()

    payload: Dict[str, Any] = {
        "provider": "claude",
        "status": "missing",
        "head_sha": args.head_sha,
        "findings": [],
        "errors": [],
        "source": {},
    }

    if not token:
        payload["status"] = "error"
        payload["errors"].append(f"missing token in env var: {args.token_env}")
        _write_json(args.out, payload)
        return 1

    contract = json.loads(Path(args.contract).read_text(encoding="utf-8"))
    provider_cfg = (((contract.get("reviewProviders") or {}).get("providers") or {}).get("claude") or {})

    marker = str(provider_cfg.get("marker", "<!-- claude-review-findings -->"))
    weak_confidence = float(provider_cfg.get("weakConfidenceThreshold", 0.55))
    keywords = [str(k).lower() for k in provider_cfg.get("actionableSummaryKeywords", [])]

    github_cfg = provider_cfg.get("github", {}) if isinstance(provider_cfg.get("github", {}), dict) else {}
    trusted_app_ids_env = str(github_cfg.get("trustedAppIdsEnv", "OPENFANG_CLAUDE_TRUSTED_APP_IDS"))
    trusted_app_ids = _parse_int_csv(os.getenv(trusted_app_ids_env, ""))
    trusted_logins = [str(item).strip().lower() for item in github_cfg.get("trustedLogins", [])]
    maintainer_allowlist = [str(item).strip().lower() for item in github_cfg.get("maintainerAllowlist", [])]

    payload["trusted_sources"] = {
        "trusted_app_ids": trusted_app_ids,
        "trusted_logins": trusted_logins,
        "maintainer_allowlist": maintainer_allowlist,
    }

    latest: Optional[Tuple[Dict[str, Any], Dict[str, Any]]] = None

    try:
        for source in _iter_comment_sources(args.repo, args.pr, token):
            body = str(source.get("body", ""))
            blob = _extract_payload_after_marker(body, marker)
            if not blob:
                continue

            if not _is_trusted_source(
                source,
                trusted_app_ids=trusted_app_ids,
                trusted_logins=trusted_logins,
                maintainer_allowlist=maintainer_allowlist,
            ):
                payload["errors"].append(
                    f"ignored untrusted claude payload from {(source.get('user') or {}).get('login', 'unknown')}"
                )
                continue

            source_sha = str(blob.get("head_sha", "")).strip()
            if source_sha != args.head_sha:
                payload["errors"].append(f"ignored stale claude payload for sha:{source_sha}")
                continue

            source_id = int(source.get("id", 0) or 0)
            if latest is None or source_id > int(latest[1].get("id", 0) or 0):
                latest = (blob, source)

    except urllib.error.HTTPError as exc:
        payload["status"] = "error"
        payload["errors"].append(f"GitHub API HTTP error while ingesting Claude feedback: {exc.code}")
        _write_json(args.out, payload)
        return 1
    except urllib.error.URLError as exc:
        payload["status"] = "error"
        payload["errors"].append(f"GitHub API connection error while ingesting Claude feedback: {exc.reason}")
        _write_json(args.out, payload)
        return 1
    except Exception as exc:
        payload["status"] = "error"
        payload["errors"].append(f"unexpected Claude ingestion error: {exc}")
        _write_json(args.out, payload)
        return 1

    if latest is None:
        payload["status"] = "missing"
        payload["errors"].append("no trusted current-head Claude feedback payload found")
        _write_json(args.out, payload)
        return 2

    finding_blob, source = latest
    findings = finding_blob.get("findings", [])
    if not isinstance(findings, list):
        findings = []

    payload["status"] = str(finding_blob.get("status", "success")).lower()
    payload["findings"] = [
        _normalize_finding(
            finding if isinstance(finding, dict) else {},
            weak_confidence_threshold=weak_confidence,
            actionable_keywords=keywords,
            index=index,
        )
        for index, finding in enumerate(findings, start=1)
    ]
    payload["source"] = {
        "source_type": str(source.get("_source_type", "comment")),
        "id": source.get("id"),
        "author": (source.get("user") or {}).get("login", ""),
        "author_app_id": (source.get("performed_via_github_app") or {}).get("id"),
        "trusted": True,
    }

    _write_json(args.out, payload)
    return 0 if payload["status"] == "success" else 2


if __name__ == "__main__":
    raise SystemExit(main())
