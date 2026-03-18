#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

repair=0
if [[ "${1:-}" == "--repair" ]]; then
  repair=1
fi

cleanup_stale_locks
git -C "$(repo_root)" worktree prune >/dev/null 2>&1 || true

echo "OpenFang worktree recovery"
echo "repo: $(repo_root)"
echo "policy-root: $(policy_root)"
echo

if root_has_disallowed_dirt; then
  echo "root checkout has disallowed dirt:"
  root_disallowed_dirty_files | sed 's/^/ - /'
else
  echo "root checkout is clean enough for policy lock"
fi

echo
echo "active lock files:"
found_lock=0
shopt -s nullglob
for lock_file in "$(lock_root)"/*.env; do
  found_lock=1
  state="$(lock_state "$lock_file")"
  load_lock_file "$lock_file" || continue
  printf ' - %s (%s) -> %s\n' "$(basename "$lock_file")" "$state" "${LOCK_WORKTREE:-unknown}"
done
if [[ "$found_lock" == "0" ]]; then
  echo " - none"
fi

echo
echo "managed worktrees:"
git -C "$(repo_root)" worktree list --porcelain | awk '
  /^worktree / { path=substr($0,10) }
  /^branch refs\/heads\// { branch=substr($0,19) }
  /^prunable/ { prunable="yes" }
  /^$/ {
    if (path != "") {
      branch_label = (branch == "" ? "detached" : branch)
      prunable_label = (prunable == "yes" ? " prunable" : "")
      printf " - %s [%s]%s\n", path, branch_label, prunable_label
    }
    path=""; branch=""; prunable=""
  }
'

if [[ "$repair" == "1" ]]; then
  echo
  echo "repair actions:"
  cleanup_stale_locks
  git -C "$(repo_root)" worktree prune >/dev/null 2>&1 || true
  echo " - pruned stale git worktree entries"
  echo " - removed stale OpenFang lock files"
fi
