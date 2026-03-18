#!/usr/bin/env python3
"""Validate OpenFang git remote policy and credential hygiene."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path
from typing import Dict, List


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate git remotes for OpenFang policy")
    parser.add_argument("--repo", default=".", help="Repository path")
    parser.add_argument("--json", action="store_true", help="Emit JSON")
    parser.add_argument("--require-canonical", default="myfork", help="Expected canonical remote name")
    return parser.parse_args()


def _run(repo: str, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-C", repo, *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return proc.stdout


def _has_embedded_credentials(url: str) -> bool:
    return bool(re.search(r"^https://[^/@]+@", url))


def _redact_url(url: str) -> str:
    return re.sub(r"^(https://)[^/@]+@", r"\1<redacted>@", url)


def main() -> int:
    args = parse_args()
    repo = str(Path(args.repo).resolve())
    remotes: Dict[str, Dict[str, str]] = {}

    for line in _run(repo, "remote", "-v").splitlines():
        parts = line.split()
        if len(parts) < 3:
            continue
        name, url, kind = parts[0], parts[1], parts[2].strip("()")
        remotes.setdefault(name, {})[kind] = url

    findings: List[str] = []
    for name, urls in remotes.items():
        for kind, url in urls.items():
            if _has_embedded_credentials(url):
                findings.append(
                    f"{name} {kind} remote embeds credentials: {_redact_url(url)}"
                )

    if args.require_canonical not in remotes:
        findings.append(f"missing canonical remote: {args.require_canonical}")

    if "origin" not in remotes:
        findings.append("missing upstream remote: origin")

    payload = {
        "repo": repo,
        "canonical_remote": args.require_canonical,
        "remotes": remotes,
        "ok": not findings,
        "findings": findings,
    }

    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(f"repo: {repo}")
        print(f"canonical remote: {args.require_canonical}")
        for name, urls in sorted(remotes.items()):
            for kind, url in sorted(urls.items()):
                print(f" - {name} ({kind}): {url}")
        if findings:
            print("findings:")
            for finding in findings:
                print(f" - {finding}")

    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
