#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage:
  scripts/worktree/guard.sh session --tool <claude|codex> --cwd <path>
  scripts/worktree/guard.sh launch --tool <claude|codex> --task <task> --worktree <path>
EOF
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

command_name=""
tool=""
task=""
cwd=""
worktree=""

if [[ $# -lt 1 ]]; then
  usage >&2
  exit 2
fi

command_name="$1"
shift

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
    --cwd)
      cwd="$2"
      shift 2
      ;;
    --worktree)
      worktree="$2"
      shift 2
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

case "$tool" in
  claude|codex) ;;
  *)
    fail "tool must be claude or codex"
    ;;
esac

cleanup_stale_locks

case "$command_name" in
  session)
    [[ -n "$cwd" ]] || fail "--cwd is required for session checks"
    is_openfang_context "$cwd" || exit 0
    is_root_checkout "$cwd" && fail "OpenFang policy: root checkout is inspection-only. Use of-$tool <task-name>."

    top="$(canonical_path "$(git_top "$cwd")")"
    branch="$(current_branch "$cwd")"
    [[ "$branch" == "$tool/"* ]] || fail "OpenFang policy: $tool must run on a $tool/<task> branch."

    task_slug="${branch#"$tool/"}"
    expected_top="$(canonical_path "$(expected_worktree_path "$tool" "$task_slug")")"
    [[ "$top" == "$expected_top" ]] || fail "OpenFang policy: $tool must run from $expected_top."

    lock_file="$(lock_file_path "$tool" "$task_slug")"
    load_lock_file "$lock_file" || fail "OpenFang policy: session must start through of-$tool so a worktree lock exists."
    lock_pid_alive "$lock_file" || fail "OpenFang policy: lock is stale; restart via of-$tool $task_slug."
    [[ "${LOCK_WORKTREE:-}" == "$top" ]] || fail "OpenFang policy: lock/worktree mismatch for $tool/$task_slug."
    ;;
  launch)
    [[ -n "$task" ]] || fail "--task is required for launch checks"
    [[ -n "$worktree" ]] || fail "--worktree is required for launch checks"

    task_slug="$(slugify "$task")"
    top="$(canonical_path "$(git_top "$worktree")")"
    branch="$(current_branch "$worktree")"
    expected_top="$(canonical_path "$(expected_worktree_path "$tool" "$task_slug")")"
    expected_ref="$(expected_branch "$tool" "$task_slug")"

    [[ "$top" == "$expected_top" ]] || fail "OpenFang policy: expected worktree at $expected_top."
    [[ "$branch" == "$expected_ref" ]] || fail "OpenFang policy: expected branch $expected_ref, found $branch."
    worktree_is_clean "$worktree" || fail "OpenFang policy: refusing to start in a dirty worktree."

    conflict="$(scan_conflicting_lock "$top" "$tool" "$task_slug" || true)"
    [[ -z "$conflict" ]] || fail "OpenFang policy: worktree is already locked by another live session."
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
