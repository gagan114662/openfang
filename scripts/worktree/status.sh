#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

REPO_ROOT="$(repo_root)"

print_worktree_row() {
  local path="$1"
  local branch="$2"
  local head="$3"
  local note="$4"
  local tool="other"
  local state="clean"
  local lock="n/a"
  local place=""
  local task lock_file

  [[ -n "$path" ]] || return 0

  if [[ -z "$branch" ]]; then
    branch="detached:${head:0:12}"
  fi

  case "$branch" in
    claude/*)
      tool="claude"
      task="${branch#claude/}"
      ;;
    codex/*)
      tool="codex"
      task="${branch#codex/}"
      ;;
  esac

  if [[ "$path" == "$REPO_ROOT" ]]; then
    place="root"
  else
    place="worktree"
  fi

  if [[ -n "$note" ]]; then
    state="$note"
  elif [[ ! -d "$path" ]]; then
    state="missing"
  else
    local dirty_count
    dirty_count="$(worktree_dirty_count "$path")"
    if [[ "$dirty_count" != "0" ]]; then
      state="dirty:$dirty_count"
    fi
  fi

  if [[ "$tool" != "other" ]]; then
    lock_file="$(lock_file_path "$tool" "$task")"
    lock="$(lock_state "$lock_file")"
  fi

  printf '%-7s %-10s %-8s %-24s %-7s %s\n' "$tool" "$state" "$lock" "$branch" "$place" "$path"
}

print_lock_row() {
  local lock_file="$1"
  load_lock_file "$lock_file" || return 0
  printf '%-7s %-24s %-8s %-8s %s\n' \
    "${LOCK_TOOL:-?}" \
    "${LOCK_TASK:-?}" \
    "$(lock_state "$lock_file")" \
    "${LOCK_PID:-?}" \
    "${LOCK_WORKTREE:-?}"
}

cleanup_stale_locks

printf 'root-lock: %s\n' "$(root_lock_state)"
printf 'policy-root: %s\n' "$(policy_root)"
printf '\n'

echo "tool    state      lock     branch                   place   path"
echo "------- ---------- -------- ------------------------ ------- ----"

path=""
branch=""
head=""
note=""
while IFS= read -r line; do
  if [[ -z "$line" ]]; then
    print_worktree_row "$path" "$branch" "$head" "$note"
    path=""
    branch=""
    head=""
    note=""
    continue
  fi

  case "$line" in
    worktree\ *) path="${line#worktree }" ;;
    branch\ refs/heads/*) branch="${line#branch refs/heads/}" ;;
    HEAD\ *) head="${line#HEAD }" ;;
    prunable) note="prunable" ;;
    locked) note="git-locked" ;;
  esac
done < <(git -C "$REPO_ROOT" worktree list --porcelain && printf '\n')

printf '\n'
echo "tool    task                     lock     pid      worktree"
echo "------- ------------------------ -------- -------- --------"

shopt -s nullglob
for lock_file in "$(lock_root)"/*.env; do
  print_lock_row "$lock_file"
done
