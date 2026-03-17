#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: scripts/worktree/finish_agent_task.sh [--tool <claude|codex>] [--task <task>] [--worktree <path>] [--lock-file <path>]
EOF
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

tool="${OPENFANG_AGENT_TOOL:-}"
task="${OPENFANG_AGENT_TASK:-}"
worktree="${OPENFANG_AGENT_WORKTREE:-}"
lock_file="${OPENFANG_AGENT_LOCK_FILE:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tool)
      tool="$2"
      shift 2
      ;;
    --task)
      task="$2"
      shift 2
      ;;
    --worktree)
      worktree="$2"
      shift 2
      ;;
    --lock-file)
      lock_file="$2"
      shift 2
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$worktree" ]]; then
  worktree="$(git rev-parse --show-toplevel)"
fi
worktree="$(canonical_path "$worktree")"

if [[ -z "$tool" || -z "$task" ]]; then
  branch="$(current_branch "$worktree")"
  case "$branch" in
    claude/*)
      tool="claude"
      task="${branch#claude/}"
      ;;
    codex/*)
      tool="codex"
      task="${branch#codex/}"
      ;;
    *)
      fail "OpenFang finish gate: cannot infer tool/task from branch $branch."
      ;;
  esac
fi

[[ -n "$lock_file" ]] || lock_file="$(lock_file_path "$tool" "$task")"
load_lock_file "$lock_file" || fail "OpenFang finish gate: missing session lock at $lock_file."

[[ "${LOCK_WORKTREE:-}" == "$worktree" ]] || fail "OpenFang finish gate: lock points to ${LOCK_WORKTREE:-unknown}, not $worktree."
[[ "$(current_branch "$worktree")" == "$(expected_branch "$tool" "$task")" ]] || fail "OpenFang finish gate: branch mismatch."
worktree_is_clean "$worktree" || fail "OpenFang finish gate: worktree is still dirty. Commit or clean it before finishing."

base_head="${LOCK_START_HEAD:-}"
[[ -n "$base_head" ]] || fail "OpenFang finish gate: lock is missing START_HEAD metadata."

changed_files="$(session_changed_files "$worktree" "$base_head" || true)"
if [[ -z "$changed_files" ]]; then
  rm -f "$lock_file"
  printf 'OpenFang finish gate: no session changes; lock cleared.\n'
  exit 0
fi

if session_is_docs_only "$worktree" "$base_head"; then
  rm -f "$lock_file"
  printf 'OpenFang finish gate: docs-only session; lock cleared without cargo validation.\n'
  exit 0
fi

(
  cd "$worktree"
  cargo build --workspace --lib
  cargo test --workspace
)

rm -f "$lock_file"
printf 'OpenFang finish gate: validation passed; lock cleared.\n'
