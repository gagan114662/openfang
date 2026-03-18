#!/usr/bin/env python3
"""Configure GitHub Actions variables for OpenFang PR review automation."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path
from typing import Any, Iterable, List, Optional, Sequence, Set


CLAUDE_MARKER = "<!-- claude-review-findings -->"
DEFAULT_REMEDIATION_CMD = (
    "codex exec "
    "\"Read artifacts/claude-findings.json and fix all actionable findings with minimal safe changes. "
    "Obey .harness/policy.contract.json remediationPolicy, run required validation, and stop when done.\""
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Configure OpenFang PR review automation variables")
    parser.add_argument("--repo", default="", help="GitHub repo in owner/repo form; defaults from git remote origin")
    parser.add_argument("--pr", type=int, default=0, help="PR number to inspect for Claude GitHub App IDs")
    parser.add_argument(
        "--search-pr-limit",
        type=int,
        default=20,
        help="How many open PRs to scan when auto-discovering Claude GitHub App IDs without --pr",
    )
    parser.add_argument(
        "--remediation-cmd",
        default=DEFAULT_REMEDIATION_CMD,
        help="Value to store in OPENFANG_REMEDIATION_CMD",
    )
    parser.add_argument(
        "--claude-app-id",
        action="append",
        default=[],
        help="Claude GitHub App ID to trust (repeatable). If omitted, inspect PR comments/reviews.",
    )
    parser.add_argument(
        "--skip-remediation-cmd",
        action="store_true",
        help="Skip setting OPENFANG_REMEDIATION_CMD",
    )
    parser.add_argument(
        "--skip-claude-app-ids",
        action="store_true",
        help="Skip setting OPENFANG_CLAUDE_TRUSTED_APP_IDS",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print intended variable updates without calling gh variable set",
    )
    return parser.parse_args()


def _run(args: Sequence[str], *, cwd: Optional[str] = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, check=True, capture_output=True, text=True, cwd=cwd)


def _repo_from_remote() -> str:
    url = _run(["git", "config", "--get", "remote.origin.url"]).stdout.strip()
    if not url:
        raise RuntimeError("remote.origin.url is not configured")

    normalized = url
    if normalized.endswith(".git"):
        normalized = normalized[:-4]

    if normalized.startswith("git@github.com:"):
        return normalized.split("git@github.com:", 1)[1]

    marker = "github.com/"
    if marker in normalized:
        return normalized.split(marker, 1)[1]

    raise RuntimeError(f"could not parse GitHub repo from remote origin URL: {url}")


def _gh_api_json(path: str) -> Any:
    payload = _run(["gh", "api", path]).stdout
    return json.loads(payload)


def _list_open_pr_numbers(repo: str, limit: int) -> List[int]:
    payload = _run(
        [
            "gh",
            "pr",
            "list",
            "-R",
            repo,
            "--limit",
            str(limit),
            "--json",
            "number",
        ]
    ).stdout
    parsed = json.loads(payload)
    return [int(item["number"]) for item in parsed if isinstance(item, dict) and "number" in item]


def _extract_app_id(entry: dict[str, Any]) -> Optional[int]:
    app = entry.get("performed_via_github_app") or {}
    app_id = app.get("id")
    try:
        return int(app_id)
    except (TypeError, ValueError):
        return None


def _body_contains_marker(entry: dict[str, Any], marker: str) -> bool:
    return marker in str(entry.get("body", ""))


def discover_claude_app_ids(repo: str, pr_number: int, marker: str = CLAUDE_MARKER) -> List[int]:
    if pr_number <= 0:
        raise RuntimeError("a positive --pr is required when auto-discovering Claude app IDs")

    issue_comments = _gh_api_json(f"repos/{repo}/issues/{pr_number}/comments?per_page=100")
    reviews = _gh_api_json(f"repos/{repo}/pulls/{pr_number}/reviews?per_page=100")

    discovered: Set[int] = set()
    for entry in list(issue_comments) + list(reviews):
        if not isinstance(entry, dict):
            continue
        if not _body_contains_marker(entry, marker):
            continue
        app_id = _extract_app_id(entry)
        if app_id is not None:
            discovered.add(app_id)

    return sorted(discovered)


def discover_claude_app_ids_from_open_prs(repo: str, limit: int, marker: str = CLAUDE_MARKER) -> List[int]:
    for pr_number in _list_open_pr_numbers(repo, limit):
        app_ids = discover_claude_app_ids(repo, pr_number, marker=marker)
        if app_ids:
            return app_ids
    return []


def _set_repo_variable(repo: str, name: str, value: str, *, dry_run: bool) -> None:
    if dry_run:
        print(f"[dry-run] gh variable set {name} -R {repo} --body <redacted>")
        print(f"  value={value}")
        return

    _run(["gh", "variable", "set", name, "-R", repo, "--body", value])
    print(f"set {name} on {repo}")


def _normalize_app_ids(values: Iterable[str]) -> List[int]:
    app_ids: Set[int] = set()
    for value in values:
        for token in str(value).split(","):
            stripped = token.strip()
            if not stripped:
                continue
            try:
                app_ids.add(int(stripped))
            except ValueError as exc:
                raise RuntimeError(f"invalid app id: {stripped}") from exc
    return sorted(app_ids)


def main() -> int:
    args = parse_args()
    repo = args.repo or _repo_from_remote()

    if not args.skip_remediation_cmd:
        remediation_cmd = args.remediation_cmd.strip()
        if not remediation_cmd:
            raise RuntimeError("remediation command cannot be empty")
        _set_repo_variable(repo, "OPENFANG_REMEDIATION_CMD", remediation_cmd, dry_run=args.dry_run)

    if not args.skip_claude_app_ids:
        app_ids = _normalize_app_ids(args.claude_app_id)
        if not app_ids:
            if args.pr > 0:
                app_ids = discover_claude_app_ids(repo, args.pr)
            else:
                app_ids = discover_claude_app_ids_from_open_prs(repo, args.search_pr_limit)
        if not app_ids:
            raise RuntimeError(
                "could not discover Claude GitHub App IDs from recent PRs; pass --pr or --claude-app-id explicitly"
            )
        _set_repo_variable(
            repo,
            "OPENFANG_CLAUDE_TRUSTED_APP_IDS",
            ",".join(str(app_id) for app_id in app_ids),
            dry_run=args.dry_run,
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
