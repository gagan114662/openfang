#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: agent_entry.sh <claude|codex> <real-binary> [args...]" >&2
  exit 2
fi

TOOL="$1"
REAL_BIN="$2"
shift 2

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"

if [[ "${OPENFANG_AGENT_LAUNCHER:-}" == "1" ]]; then
  exec "$REAL_BIN" "$@"
fi

if ! git_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
  exec "$REAL_BIN" "$@"
fi

common_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
repo_common_dir="$(git -C "$OPENFANG_POLICY_REPO_ROOT" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"

if [[ -z "$common_dir" || -z "$repo_common_dir" || "$common_dir" != "$repo_common_dir" ]]; then
  exec "$REAL_BIN" "$@"
fi

branch="$(git -C "$git_root" rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
if [[ "$branch" == "$TOOL/"* ]]; then
  exec "$REAL_BIN" "$@"
fi

printf 'OpenFang %s launcher detected repo context.\n' "$TOOL" >&2
read -r -p "Task name: " task_name
if [[ -z "${task_name// }" ]]; then
  echo "error: task name is required" >&2
  exit 1
fi

exec "$OPENFANG_POLICY_REPO_ROOT/scripts/worktree/run_agent_tool.sh" "$TOOL" "$task_name" -- "$@"
