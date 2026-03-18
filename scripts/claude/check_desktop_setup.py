#!/usr/bin/env python3
"""Verify the Claude Desktop cowork setup for the current OpenFang checkout."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict, List


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Check Claude Desktop readiness for OpenFang")
    parser.add_argument("--repo", default=".", help="Repository root or worktree path")
    parser.add_argument("--json", action="store_true", help="Emit JSON")
    return parser.parse_args()


def load_json(path: Path) -> Dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
        return payload if isinstance(payload, dict) else {}
    except Exception:
        return {}


def resolve_launcher(command_name: str) -> str:
    resolved = shutil.which(command_name)
    if resolved:
        return resolved
    fallback = Path.home() / ".openfang" / "bin" / command_name
    if fallback.exists() and fallback.is_file():
        return str(fallback)
    return ""


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).resolve()
    checks: List[Dict[str, Any]] = []

    mcp_path = repo / ".mcp.json"
    mcp_payload = load_json(mcp_path) if mcp_path.exists() else {}
    servers = sorted(((mcp_payload.get("mcpServers") or {}) if isinstance(mcp_payload.get("mcpServers"), dict) else {}).keys())
    checks.append({"check": "repo_mcp_manifest", "ok": mcp_path.exists() and {"openfang", "contextplus"}.issubset(set(servers)), "servers": servers})

    claude_settings_path = repo / ".claude" / "settings.json"
    claude_settings = load_json(claude_settings_path)
    hooks = claude_settings.get("hooks") if isinstance(claude_settings.get("hooks"), dict) else {}
    hooks_ok = bool(hooks)
    if not hooks_ok:
        launcher_script = repo / "scripts" / "worktree" / "run_agent_tool.sh"
        launcher_text = launcher_script.read_text(encoding="utf-8") if launcher_script.exists() else ""
        hooks_ok = all(
            needle in launcher_text
            for needle in (
                '"SessionStart"',
                '"SessionEnd"',
                '"PreToolUse"',
                '"PostToolUse"',
                "claude_hook.py",
            )
        )
    checks.append(
        {
            "check": "claude_hooks",
            "ok": hooks_ok,
            "hook_groups": sorted(hooks.keys()),
            "path": str(claude_settings_path),
        }
    )

    local_settings_path = repo / ".claude" / "settings.local.json"
    local_settings = load_json(local_settings_path)
    local_servers = local_settings.get("enabledMcpjsonServers")
    local_ready = bool(
        local_settings.get("enableAllProjectMcpServers") is True
        and isinstance(local_servers, list)
        and {"openfang", "contextplus"}.issubset({str(item) for item in local_servers})
    )
    if not local_ready:
        launcher_script = repo / "scripts" / "worktree" / "run_agent_tool.sh"
        launcher_text = launcher_script.read_text(encoding="utf-8") if launcher_script.exists() else ""
        local_ready = all(
            needle in launcher_text
            for needle in (
                '"enableAllProjectMcpServers": true',
                '"enabledMcpjsonServers"',
                '"contextplus"',
                '"openfang"',
            )
        )
    checks.append(
        {
            "check": "claude_project_mcp_settings",
            "ok": local_ready,
            "path": str(local_settings_path),
        }
    )

    for command_name in ("of-claude", "of-codex", "claude"):
        resolved = resolve_launcher(command_name)
        checks.append({
            "check": f"path:{command_name}",
            "ok": bool(resolved),
            "resolved": resolved,
        })

    guard_path = repo / "scripts" / "worktree" / "guard.sh"
    checks.append({"check": "guard_script", "ok": guard_path.exists(), "path": str(guard_path)})

    if guard_path.exists():
        proc = subprocess.run(
            ["bash", str(guard_path), "session", "--tool", "claude", "--cwd", str(repo)],
            capture_output=True,
            text=True,
        )
        checks.append({
            "check": "guard_policy",
            "ok": proc.returncode == 0
            or "root checkout is inspection-only" in (proc.stderr or "")
            or "must run on a claude/<task> branch" in (proc.stderr or ""),
            "stdout": proc.stdout.strip(),
            "stderr": proc.stderr.strip(),
            "exit_code": proc.returncode,
        })

    ok = all(check["ok"] for check in checks)
    payload = {"repo": str(repo), "ok": ok, "checks": checks}

    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(f"Claude Desktop readiness for {repo}")
        for check in checks:
            mark = "ok" if check["ok"] else "fail"
            print(f" - {check['check']}: {mark}")

    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
