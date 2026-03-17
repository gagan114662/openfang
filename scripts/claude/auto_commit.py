#!/usr/bin/env python3
"""Auto-commit at end of Claude/Codex sessions.

Called from claude_hook.py on session-end and stop events.
Commits all staged + unstaged changes with a descriptive message.
Skips if working tree is clean.
"""
import os
import pathlib
import subprocess
import sys


def git(cmd: list[str], cwd: str) -> tuple[int, str]:
    result = subprocess.run(
        ["git"] + cmd,
        capture_output=True, text=True, timeout=10, cwd=cwd,
    )
    return result.returncode, result.stdout.strip()


def auto_commit(cwd: str) -> None:
    # Check if there are any changes
    rc, status = git(["status", "--porcelain"], cwd)
    if rc != 0 or not status.strip():
        return  # Clean tree or not a git repo

    # Get branch name for the commit message
    _, branch = git(["rev-parse", "--abbrev-ref", "HEAD"], cwd)

    # Count changes
    lines = [l for l in status.strip().split("\n") if l.strip()]
    modified = sum(1 for l in lines if l.startswith(" M") or l.startswith("M "))
    added = sum(1 for l in lines if l.startswith("??"))
    deleted = sum(1 for l in lines if l.startswith(" D") or l.startswith("D "))

    parts = []
    if modified:
        parts.append(f"{modified} modified")
    if added:
        parts.append(f"{added} new")
    if deleted:
        parts.append(f"{deleted} deleted")
    change_summary = ", ".join(parts) if parts else f"{len(lines)} changes"

    # Stage everything and commit
    git(["add", "-A"], cwd)

    # Check if staging produced anything to commit
    rc, diff = git(["diff", "--cached", "--stat"], cwd)
    if not diff.strip():
        return

    msg = f"wip({branch}): auto-save session ({change_summary})"
    rc, out = git(["commit", "-m", msg, "--no-verify"], cwd)
    if rc == 0:
        print(f"auto-commit: {msg}", file=sys.stderr)
    else:
        print(f"auto-commit failed: {out}", file=sys.stderr)


if __name__ == "__main__":
    cwd = sys.argv[1] if len(sys.argv) > 1 else os.getcwd()
    auto_commit(cwd)
