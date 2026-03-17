#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: scripts/worktree/run_agent_tool.sh <claude|codex> <task-name> [-- tool-args...]

Examples:
  scripts/worktree/run_agent_tool.sh claude sentry-triage
  scripts/worktree/run_agent_tool.sh codex ws-v1 -- exec --help
EOF
}

if [[ $# -lt 2 ]]; then
  usage >&2
  exit 1
fi

TOOL="$1"
RAW_TASK="$2"
shift 2

if [[ "${1:-}" == "--" ]]; then
  shift
fi

case "$TOOL" in
  claude|codex) ;;
  *)
    echo "error: tool must be 'claude' or 'codex'" >&2
    exit 1
    ;;
esac

TASK_SLUG="$(slugify "$RAW_TASK")"
if [[ -z "$TASK_SLUG" ]]; then
  echo "error: task name resolves to an empty slug" >&2
  exit 1
fi

REPO_ROOT="$(repo_root)"
DEST="$(expected_worktree_path "$TOOL" "$TASK_SLUG")"
LOCK_FILE=""
TOOL_STATUS=0
FINISH_STATUS=0
TOOL_BIN="$(command -v "$TOOL" || true)"

if [[ -z "$TOOL_BIN" ]]; then
  echo "error: $TOOL is not on PATH" >&2
  exit 1
fi

relock_root() {
  OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT" bash "$REPO_ROOT/scripts/worktree/root_mode.sh" lock >/dev/null
}

sync_claude_worktree_settings() {
  local dest="$1"
  local hook_path settings_dir settings_path
  hook_path="$REPO_ROOT/scripts/claude/claude_hook.py"
  settings_dir="$dest/.claude"
  settings_path="$settings_dir/settings.json"

  mkdir -p "$settings_dir"
  cat >"$settings_path" <<EOF
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Task",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' post-task"
          }
        ]
      },
      {
        "matcher": "TodoWrite",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' post-todo"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Task",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' pre-task"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' session-end"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' session-start"
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' stop"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "OPENFANG_POLICY_REPO_ROOT='$REPO_ROOT' python3 '$hook_path' user-prompt-submit"
          }
        ]
      }
    ]
  },
  "permissions": {
    "deny": [
      "Read(./.entire/metadata/**)"
    ]
  }
}
EOF
}

cleanup() {
  relock_root >/dev/null 2>&1 || true
}

trap cleanup EXIT

OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT" bash "$REPO_ROOT/scripts/worktree/root_mode.sh" lock >/dev/null
OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT" bash "$REPO_ROOT/scripts/worktree/open_agent_worktree.sh" "$TOOL" "$TASK_SLUG" >/dev/null
OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT" bash "$REPO_ROOT/scripts/worktree/guard.sh" launch --tool "$TOOL" --task "$TASK_SLUG" --worktree "$DEST"

if [[ "$TOOL" == "claude" ]]; then
  sync_claude_worktree_settings "$DEST"
fi

export OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT"
LOCK_FILE="$(acquire_session_lock "$TOOL" "$TASK_SLUG" "$$" "$DEST")" || {
  echo "OpenFang policy: unable to acquire a live session lock for $TOOL/$TASK_SLUG." >&2
  exit 1
}

set +e
(
  cd "$DEST"
  export OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT"
  export OPENFANG_AGENT_LAUNCHER=1
  export OPENFANG_AGENT_TOOL="$TOOL"
  export OPENFANG_AGENT_TASK="$TASK_SLUG"
  export OPENFANG_AGENT_WORKTREE="$DEST"
  export OPENFANG_AGENT_LOCK_FILE="$LOCK_FILE"
  "$TOOL_BIN" "$@"
)
TOOL_STATUS=$?
set -e

set +e
OPENFANG_POLICY_REPO_ROOT="$REPO_ROOT" bash "$REPO_ROOT/scripts/worktree/finish_agent_task.sh" \
  --tool "$TOOL" \
  --task "$TASK_SLUG" \
  --worktree "$DEST" \
  --lock-file "$LOCK_FILE"
FINISH_STATUS=$?
set -e

if [[ "$FINISH_STATUS" -ne 0 ]]; then
  exit "$FINISH_STATUS"
fi

exit "$TOOL_STATUS"
