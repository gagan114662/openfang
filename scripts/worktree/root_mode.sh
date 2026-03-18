#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: scripts/worktree/root_mode.sh <lock|unlock|status>
EOF
}

ROOT_MUTEX_PATH=""

root_mutex_path() {
  printf '%s\n' "$(root_lock_root)/mutex.lock"
}

release_root_mutex() {
  [[ -n "$ROOT_MUTEX_PATH" ]] || return 0
  rm -rf "$ROOT_MUTEX_PATH"
  ROOT_MUTEX_PATH=""
}

acquire_root_mutex() {
  local path owner tries
  ensure_policy_dirs
  path="$(root_mutex_path)"
  tries=0

  while ! mkdir "$path" 2>/dev/null; do
    if [[ -f "$path/pid" ]]; then
      owner="$(cat "$path/pid" 2>/dev/null || true)"
      if [[ -n "$owner" ]] && ! ps -p "$owner" >/dev/null 2>&1; then
        rm -rf "$path"
        continue
      fi
    fi

    tries=$((tries + 1))
    if [[ "$tries" -ge 120 ]]; then
      echo "OpenFang root lock: timed out waiting for root lock mutex." >&2
      exit 1
    fi
    sleep 1
  done

  printf '%s\n' "$$" >"$path/pid"
  ROOT_MUTEX_PATH="$path"
  trap release_root_mutex EXIT
}

lock_root_checkout() {
  ensure_policy_dirs

  local manifest status_file repo
  manifest="$(root_lock_manifest_path)"
  status_file="$(root_lock_status_path)"
  repo="$(repo_root)"

  if root_locked; then
    printf 'locked\n'
    return 0
  fi

  if root_has_disallowed_dirt; then
    echo "OpenFang policy: cannot lock the root checkout with disallowed uncommitted changes." >&2
    root_disallowed_dirty_files | sed 's/^/ - /' >&2
    return 1
  fi

  sync_canonical_root_branch

  rm -f "$manifest"
  touch "$manifest"

  local dirs_tmp
  dirs_tmp="$(mktemp)"

  while IFS= read -r -d '' rel; do
    local abs file_mode dir
    abs="$repo/$rel"
    [[ -e "$abs" ]] || continue
    file_mode="$(stat -f '%Lp' "$abs")"
    printf 'F\t%s\t%s\n' "$file_mode" "$abs" >>"$manifest"

    dir="$(dirname "$abs")"
    while [[ "$dir" == "$repo"* ]]; do
      printf '%s\n' "$dir" >>"$dirs_tmp"
      [[ "$dir" == "$repo" ]] && break
      dir="$(dirname "$dir")"
    done
  done < <(git -C "$repo" ls-files -z)

  sort -u "$dirs_tmp" | while IFS= read -r dir; do
    [[ -d "$dir" ]] || continue
    printf 'D\t%s\t%s\n' "$(stat -f '%Lp' "$dir")" "$dir" >>"$manifest"
  done
  rm -f "$dirs_tmp"

  while IFS=$'\t' read -r kind mode path; do
    [[ "$kind" == "D" ]] || continue
    chmod a-w "$path"
  done <"$manifest"

  while IFS=$'\t' read -r kind mode path; do
    [[ "$kind" == "F" ]] || continue
    chmod a-w "$path"
  done <"$manifest"

  write_root_lock_status "locked"
  printf 'locked\n'
}

unlock_root_checkout() {
  local manifest
  manifest="$(root_lock_manifest_path)"

  if [[ ! -f "$manifest" ]]; then
    printf 'unlocked\n'
    return 0
  fi

  while IFS=$'\t' read -r kind mode path; do
    [[ "$kind" == "F" ]] || continue
    [[ -e "$path" ]] || continue
    chmod "$mode" "$path"
  done <"$manifest"

  while IFS=$'\t' read -r kind mode path; do
    [[ "$kind" == "D" ]] || continue
    [[ -d "$path" ]] || continue
    chmod "$mode" "$path"
  done <"$manifest"

  rm -f "$manifest" "$(root_lock_status_path)"
  printf 'unlocked\n'
}

status_root_checkout() {
  printf '%s\n' "$(root_lock_state)"
}

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 2
fi

case "$1" in
  lock)
    acquire_root_mutex
    lock_root_checkout
    ;;
  unlock)
    acquire_root_mutex
    unlock_root_checkout
    ;;
  status)
    status_root_checkout
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
