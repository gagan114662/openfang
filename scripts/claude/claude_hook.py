#!/usr/bin/env python3
import json
import os
import pathlib
import subprocess
import sys
import urllib.error
import urllib.request


REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
ARTIFACT_PATH = REPO_ROOT / "artifacts" / "claude" / "hook-events.jsonl"
CONTRACTS_DIR = REPO_ROOT / "contracts"


def load_payload(raw: bytes):
    if not raw.strip():
        return {}
    try:
        return json.loads(raw.decode("utf-8"))
    except Exception:
        return {"raw_stdin": raw.decode("utf-8", errors="replace")}


def daemon_base_url() -> str:
    explicit = os.environ.get("OPENFANG_API_BASE")
    if explicit:
        return explicit.rstrip("/")

    daemon_info = pathlib.Path.home() / ".openfang" / "daemon.json"
    if daemon_info.exists():
        try:
            data = json.loads(daemon_info.read_text())
            listen_addr = data.get("listen_addr", "")
            if listen_addr:
                if listen_addr.startswith("http://") or listen_addr.startswith("https://"):
                    return listen_addr.rstrip("/")
                return f"http://{listen_addr}"
        except Exception:
            pass
    return "http://127.0.0.1:50051"


def hook_event_kind(hook_name: str, payload: dict, return_code: int) -> tuple[str, str]:
    hook_event = (
        payload.get("data", {}).get("hookEvent")
        or payload.get("hookEvent")
        or hook_name
    )
    failed = "Failure" in str(hook_event) or return_code != 0
    mapping = {
        "session-start": "claude.session.started",
        "session-end": "claude.session.ended",
        "stop": "claude.session.stopped",
        "pre-task": "claude.task.started",
        "post-task": "claude.task.completed",
        "post-todo": "claude.task.todo_updated",
        "user-prompt-submit": "claude.prompt.submitted",
    }
    event_kind = mapping.get(hook_name, f"claude.hook.{hook_name.replace('-', '_')}")
    if failed and event_kind.endswith(".completed"):
        event_kind = event_kind[:-10] + ".failed"
    outcome = "error" if failed else "success"
    if hook_name == "pre-task":
        outcome = "started"
    return event_kind, outcome


def git_context(cwd: str) -> dict:
    """Detect git branch, worktree type, and matching contract."""
    ctx = {}
    try:
        branch = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            capture_output=True, text=True, timeout=2, cwd=cwd,
        ).stdout.strip()
        if branch:
            ctx["git.branch"] = branch

        # Detect worktree agent: claude/* or codex/* branch naming
        if branch.startswith("claude/"):
            ctx["worktree.agent"] = "claude"
            ctx["worktree.task"] = branch[len("claude/"):]
        elif branch.startswith("codex/"):
            ctx["worktree.agent"] = "codex"
            ctx["worktree.task"] = branch[len("codex/"):]

        # Match branch slug to a contract file (e.g. "kill-panics" → "01-kill-panics.md")
        if CONTRACTS_DIR.is_dir() and ctx.get("worktree.task"):
            task_slug = ctx["worktree.task"]
            for f in sorted(CONTRACTS_DIR.iterdir()):
                if f.suffix == ".md" and task_slug in f.stem:
                    ctx["contract.file"] = f.name
                    ctx["contract.id"] = f.stem.split("-", 1)[0]
                    break

        # Detect if we're in a linked worktree
        git_dir = subprocess.run(
            ["git", "rev-parse", "--git-dir"],
            capture_output=True, text=True, timeout=2, cwd=cwd,
        ).stdout.strip()
        ctx["worktree.is_linked"] = "/worktrees/" in git_dir
    except Exception:
        pass
    return ctx


def write_artifact(record: dict) -> None:
    ARTIFACT_PATH.parent.mkdir(parents=True, exist_ok=True)
    with ARTIFACT_PATH.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(record, ensure_ascii=True) + "\n")


def post_structured_event(record: dict) -> None:
    request = {
        "body": record["event_kind"],
        "level": "warn" if record["outcome"] == "error" else "info",
        "attributes": {
            "event.kind": record["event_kind"],
            "outcome": record["outcome"],
            "hook.name": record["hook_name"],
            "session.id": record.get("session_id", ""),
            "payload.hook": record["payload"],
        },
    }
    if record.get("cwd"):
        request["attributes"]["workspace.cwd"] = record["cwd"]
    if record.get("tool_use_id"):
        request["attributes"]["tool.use_id"] = record["tool_use_id"]
    if record.get("failure_reason"):
        request["attributes"]["failure_reason"] = record["failure_reason"]
    # Inject git/worktree/contract context
    for k, v in record.get("git_context", {}).items():
        request["attributes"][k] = v

    body = json.dumps(request).encode("utf-8")
    req = urllib.request.Request(
        f"{daemon_base_url()}/api/telemetry/structured",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=2):
            return
    except (urllib.error.URLError, TimeoutError):
        return


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: claude_hook.py <hook-name>", file=sys.stderr)
        return 2

    hook_name = sys.argv[1]
    raw = sys.stdin.buffer.read()
    payload = load_payload(raw)

    result = subprocess.run(
        ["entire", "hooks", "claude-code", hook_name],
        input=raw,
        cwd=REPO_ROOT,
    )

    event_kind, outcome = hook_event_kind(hook_name, payload, result.returncode)
    session_id = (
        payload.get("session_id")
        or payload.get("sessionId")
        or payload.get("session", {}).get("id")
        or os.environ.get("CLAUDE_SESSION_ID")
        or os.environ.get("SESSION_ID")
        or ""
    )
    cwd = payload.get("cwd") or os.getcwd()
    record = {
        "event_kind": event_kind,
        "outcome": outcome,
        "hook_name": hook_name,
        "session_id": session_id,
        "cwd": cwd,
        "tool_use_id": payload.get("toolUseID") or payload.get("tool_use_id"),
        "failure_reason": None if result.returncode == 0 else f"entire_exit_{result.returncode}",
        "git_context": git_context(cwd),
        "payload": payload,
    }
    write_artifact(record)
    post_structured_event(record)

    # Auto-commit on session end/stop to prevent dirty worktrees
    if hook_name in ("session-end", "stop"):
        try:
            subprocess.run(
                [sys.executable, str(REPO_ROOT / "scripts" / "claude" / "auto_commit.py"), cwd],
                timeout=10,
            )
        except Exception:
            pass

    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
