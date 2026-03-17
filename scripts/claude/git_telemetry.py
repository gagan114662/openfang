#!/usr/bin/env python3
"""Git hook telemetry — emits worktree, checkout, and merge events to OpenFang Sentry.

Install by appending to .git/hooks/post-checkout and .git/hooks/post-merge:
    python3 scripts/claude/git_telemetry.py post-checkout "$@"
    python3 scripts/claude/git_telemetry.py post-merge "$@"
"""
import json
import os
import pathlib
import subprocess
import sys
import urllib.error
import urllib.request


REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
CONTRACTS_DIR = REPO_ROOT / "contracts"


def daemon_base_url() -> str:
    explicit = os.environ.get("OPENFANG_API_BASE")
    if explicit:
        return explicit.rstrip("/")
    daemon_info = pathlib.Path.home() / ".openfang" / "daemon.json"
    if daemon_info.exists():
        try:
            data = json.loads(daemon_info.read_text())
            addr = data.get("listen_addr", "")
            if addr:
                if addr.startswith("http"):
                    return addr.rstrip("/")
                return f"http://{addr}"
        except Exception:
            pass
    return "http://127.0.0.1:50051"


def git(cmd: list[str]) -> str:
    try:
        return subprocess.run(
            ["git"] + cmd,
            capture_output=True, text=True, timeout=2, cwd=REPO_ROOT,
        ).stdout.strip()
    except Exception:
        return ""


def detect_contract(branch: str) -> dict:
    """Map branch slug to a contract file."""
    ctx = {}
    # Extract task slug from claude/<task> or codex/<task>
    parts = branch.split("/", 1)
    if len(parts) == 2 and parts[0] in ("claude", "codex"):
        task_slug = parts[1]
        ctx["worktree.agent"] = parts[0]
        ctx["worktree.task"] = task_slug
        if CONTRACTS_DIR.is_dir():
            for f in sorted(CONTRACTS_DIR.iterdir()):
                if f.suffix == ".md" and task_slug in f.stem:
                    ctx["contract.file"] = f.name
                    ctx["contract.id"] = f.stem.split("-", 1)[0]
                    break
    return ctx


def is_linked_worktree() -> bool:
    git_dir = git(["rev-parse", "--git-dir"])
    return "/worktrees/" in git_dir


def emit(event_kind: str, level: str, attrs: dict) -> None:
    request = {
        "body": event_kind,
        "level": level,
        "attributes": {"event.kind": event_kind, **attrs},
    }
    body = json.dumps(request).encode("utf-8")
    req = urllib.request.Request(
        f"{daemon_base_url()}/api/telemetry/structured",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=2):
            pass
    except (urllib.error.URLError, TimeoutError):
        pass


def handle_post_checkout(args: list[str]) -> None:
    """post-checkout receives: <prev-HEAD> <new-HEAD> <branch-flag>"""
    prev_ref = args[0] if len(args) > 0 else ""
    new_ref = args[1] if len(args) > 1 else ""
    is_branch = args[2] == "1" if len(args) > 2 else False

    if not is_branch:
        return  # file checkout, not branch switch

    branch = git(["rev-parse", "--abbrev-ref", "HEAD"])
    prev_branch = git(["name-rev", "--name-only", prev_ref]) if prev_ref else ""

    attrs = {
        "git.branch": branch,
        "git.prev_branch": prev_branch,
        "git.prev_ref": prev_ref[:12],
        "git.new_ref": new_ref[:12],
        "worktree.is_linked": is_linked_worktree(),
        "outcome": "success",
    }
    attrs.update(detect_contract(branch))
    emit("git.checkout", "info", attrs)


def handle_post_merge(args: list[str]) -> None:
    """post-merge receives: <squash-flag>"""
    is_squash = args[0] == "1" if args else False
    branch = git(["rev-parse", "--abbrev-ref", "HEAD"])
    # The merged branch is in MERGE_HEAD or can be inferred from reflog
    merge_head = git(["rev-parse", "MERGE_HEAD"]) if not is_squash else ""
    merged_branch = ""
    if merge_head:
        merged_branch = git(["name-rev", "--name-only", merge_head])

    # Detect cross-worktree merge (e.g. main merging claude/* or codex/*)
    is_cross_worktree = False
    if merged_branch:
        parts = merged_branch.split("/", 1)
        is_cross_worktree = len(parts) >= 2 and parts[0] in ("claude", "codex")

    attrs = {
        "git.branch": branch,
        "git.merged_branch": merged_branch,
        "git.merge_ref": merge_head[:12],
        "git.is_squash": is_squash,
        "worktree.is_linked": is_linked_worktree(),
        "worktree.is_cross_merge": is_cross_worktree,
        "outcome": "success",
    }
    attrs.update(detect_contract(branch))
    if merged_branch:
        attrs.update({f"merged.{k}": v for k, v in detect_contract(merged_branch).items()})

    event_kind = "git.merge.cross_worktree" if is_cross_worktree else "git.merge"
    emit(event_kind, "info", attrs)


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: git_telemetry.py <hook-name> [args...]", file=sys.stderr)
        return 2

    hook_name = sys.argv[1]
    hook_args = sys.argv[2:]

    if hook_name == "post-checkout":
        handle_post_checkout(hook_args)
    elif hook_name == "post-merge":
        handle_post_merge(hook_args)
    else:
        print(f"unknown hook: {hook_name}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
