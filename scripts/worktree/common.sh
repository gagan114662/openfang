#!/usr/bin/env bash

slugify() {
  printf '%s' "$1" \
    | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/[^a-z0-9._-]+/-/g; s/^-+//; s/-+$//; s/-+/-/g'
}

repo_root() {
  if [[ -n "${OPENFANG_POLICY_REPO_ROOT:-}" ]]; then
    printf '%s\n' "$OPENFANG_POLICY_REPO_ROOT"
    return 0
  fi
  git rev-parse --show-toplevel
}

repo_name() {
  basename "$(repo_root)"
}

policy_root() {
  printf '%s\n' "${OPENFANG_AGENT_POLICY_ROOT:-$HOME/.openfang/agent-policy/$(repo_name)}"
}

worktree_root() {
  printf '%s\n' "${OPENFANG_AGENT_WORKTREE_ROOT:-$HOME/.openfang/worktrees/$(repo_name)}"
}

lock_root() {
  printf '%s\n' "$(policy_root)/locks"
}

root_lock_root() {
  printf '%s\n' "$(policy_root)/root-lock"
}

root_lock_manifest_path() {
  printf '%s\n' "$(root_lock_root)/manifest.tsv"
}

root_lock_status_path() {
  printf '%s\n' "$(root_lock_root)/status.env"
}

ensure_policy_dirs() {
  mkdir -p "$(lock_root)" "$(root_lock_root)" "$(worktree_root)"
}

canonical_path() {
  local target="$1"
  if [[ -d "$target" ]]; then
    (cd "$target" >/dev/null 2>&1 && pwd -P)
    return 0
  fi
  local dir
  dir="$(dirname "$target")"
  printf '%s/%s\n' "$(cd "$dir" >/dev/null 2>&1 && pwd -P)" "$(basename "$target")"
}

repo_common_dir() {
  canonical_path "$(repo_root)/.git"
}

git_top() {
  git -C "$1" rev-parse --show-toplevel 2>/dev/null
}

git_common_dir() {
  local cwd="$1"
  local raw
  raw="$(git -C "$cwd" rev-parse --git-common-dir 2>/dev/null)" || return 1
  if [[ "$raw" = /* ]]; then
    canonical_path "$raw"
  else
    canonical_path "$cwd/$raw"
  fi
}

is_openfang_context() {
  local cwd="$1"
  local common
  common="$(git_common_dir "$cwd")" || return 1
  [[ "$common" == "$(repo_common_dir)" ]]
}

is_root_checkout() {
  local cwd="$1"
  local top
  top="$(git_top "$cwd")" || return 1
  [[ "$(canonical_path "$top")" == "$(canonical_path "$(repo_root)")" ]]
}

expected_branch() {
  printf '%s/%s\n' "$1" "$2"
}

expected_worktree_path() {
  printf '%s/%s/%s\n' "$(worktree_root)" "$1" "$2"
}

current_branch() {
  git -C "$1" rev-parse --abbrev-ref HEAD 2>/dev/null
}

current_head() {
  git -C "$1" rev-parse HEAD 2>/dev/null
}

worktree_dirty_count() {
  git -C "$1" status --porcelain 2>/dev/null | wc -l | tr -d ' '
}

worktree_is_clean() {
  [[ "$(worktree_dirty_count "$1")" == "0" ]]
}

canonical_sync_remote() {
  local repo remote
  repo="$(repo_root)"

  if [[ -n "${OPENFANG_CANONICAL_PUSH_REMOTE:-}" ]]; then
    printf '%s\n' "$OPENFANG_CANONICAL_PUSH_REMOTE"
    return 0
  fi

  for remote in myfork fork origin; do
    if git -C "$repo" remote get-url "$remote" >/dev/null 2>&1; then
      printf '%s\n' "$remote"
      return 0
    fi
  done

  return 1
}

remote_uses_embedded_credentials() {
  [[ "$1" =~ ^https://[^/@]+@ ]]
}

redact_remote_url() {
  local url="$1"
  if remote_uses_embedded_credentials "$url"; then
    printf '%s\n' "$url" | sed -E 's#^(https://)[^/@]+@#\1<redacted>@#'
  else
    printf '%s\n' "$url"
  fi
}

git_remote_urls() {
  git -C "$(repo_root)" remote -v 2>/dev/null || true
}

credentialed_git_remotes() {
  local name url kind clean_kind
  while read -r name url kind; do
    [[ -n "$name" && -n "$url" ]] || continue
    if remote_uses_embedded_credentials "$url"; then
      clean_kind="${kind#(}"
      clean_kind="${clean_kind%)}"
      printf '%s\t%s\t%s\n' "$name" "$clean_kind" "$(redact_remote_url "$url")"
    fi
  done < <(git_remote_urls)
}

has_credentialed_git_remotes() {
  local line
  while IFS= read -r line; do
    [[ -z "$line" ]] || return 0
  done < <(credentialed_git_remotes)
  return 1
}

canonical_sync_branch() {
  printf '%s\n' "${OPENFANG_CANONICAL_PUSH_BRANCH:-main}"
}

root_dirty_allowlist_patterns() {
  local patterns="${OPENFANG_ROOT_DIRTY_ALLOWLIST:-}"
  if [[ -n "$patterns" ]]; then
    printf '%s\n' "$patterns" | tr ':' '\n'
    return 0
  fi

  cat <<'EOF'
.claude/**
.codex/**
.entire/**
artifacts/**
log/**
*.log
.DS_Store
EOF
}

root_dirty_files() {
  git -C "$(repo_root)" status --porcelain=v1 --untracked-files=all \
    | sed -E 's/^[ MARCUD?!]{2} //'
}

root_dirty_path_allowed() {
  local path="$1"
  local pattern
  while IFS= read -r pattern; do
    [[ -n "$pattern" ]] || continue
    if [[ "$path" == $pattern ]]; then
      return 0
    fi
  done < <(root_dirty_allowlist_patterns)
  return 1
}

root_disallowed_dirty_files() {
  local path
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    if ! root_dirty_path_allowed "$path"; then
      printf '%s\n' "$path"
    fi
  done < <(root_dirty_files)
}

root_has_disallowed_dirt() {
  local path
  while IFS= read -r path; do
    [[ -z "$path" ]] || return 0
  done < <(root_disallowed_dirty_files)
  return 1
}

sync_canonical_root_branch() {
  local repo remote branch current_branch_name local_head remote_head
  repo="$(repo_root)"
  branch="$(canonical_sync_branch)"
  current_branch_name="$(current_branch "$repo")"

  if [[ "$current_branch_name" != "$branch" ]]; then
    return 0
  fi

  if root_has_disallowed_dirt; then
    return 0
  fi

  remote="$(canonical_sync_remote)" || return 0
  local_head="$(current_head "$repo")"

  if ! remote_head="$(git -C "$repo" rev-parse "$remote/$branch" 2>/dev/null)"; then
    git -C "$repo" push "$remote" "$branch:$branch" >/dev/null
    return 0
  fi

  if [[ "$local_head" == "$remote_head" ]]; then
    return 0
  fi

  if git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
    git -C "$repo" push "$remote" "$branch:$branch" >/dev/null
    return 0
  fi

  echo "OpenFang policy: canonical branch '$branch' diverged from '$remote'. Pull/rebase before locking root checkout." >&2
  return 1
}

lock_file_path() {
  printf '%s/%s--%s.env\n' "$(lock_root)" "$1" "$2"
}

load_lock_file() {
  local path="$1"
  unset LOCK_TOOL LOCK_TASK LOCK_PID LOCK_BRANCH LOCK_WORKTREE LOCK_REPO_ROOT LOCK_START_HEAD LOCK_CREATED_AT
  [[ -f "$path" ]] || return 1
  # shellcheck disable=SC1090
  . "$path"
}

write_lock_file() {
  local path="$1"
  local tool="$2"
  local task="$3"
  local pid="$4"
  local branch="$5"
  local worktree="$6"
  local start_head="$7"
  local created_at="$8"
  local tmp
  tmp="${path}.tmp.$$"
  {
    printf 'LOCK_TOOL=%q\n' "$tool"
    printf 'LOCK_TASK=%q\n' "$task"
    printf 'LOCK_PID=%q\n' "$pid"
    printf 'LOCK_BRANCH=%q\n' "$branch"
    printf 'LOCK_WORKTREE=%q\n' "$worktree"
    printf 'LOCK_REPO_ROOT=%q\n' "$(repo_root)"
    printf 'LOCK_START_HEAD=%q\n' "$start_head"
    printf 'LOCK_CREATED_AT=%q\n' "$created_at"
  } >"$tmp"
  mv "$tmp" "$path"
}

lock_pid_alive() {
  local path="$1"
  load_lock_file "$path" || return 1
  [[ -n "${LOCK_PID:-}" ]] || return 1
  ps -p "$LOCK_PID" >/dev/null 2>&1
}

clear_stale_lock_file() {
  local path="$1"
  [[ -f "$path" ]] || return 1
  if lock_pid_alive "$path"; then
    return 1
  fi
  rm -f "$path"
  return 0
}

acquire_session_lock() {
  local tool="$1"
  local task="$2"
  local pid="$3"
  local worktree="$4"
  local branch start_head path conflict

  ensure_policy_dirs
  worktree="$(canonical_path "$worktree")"
  branch="$(current_branch "$worktree")"
  start_head="$(current_head "$worktree")"
  path="$(lock_file_path "$tool" "$task")"

  cleanup_stale_locks
  conflict="$(scan_conflicting_lock "$worktree" "$tool" "$task" || true)"
  [[ -z "$conflict" ]] || return 1

  if [[ -f "$path" ]]; then
    if lock_pid_alive "$path"; then
      return 1
    fi
    rm -f "$path"
  fi

  write_lock_file "$path" "$tool" "$task" "$pid" "$branch" "$worktree" "$start_head" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s\n' "$path"
}

release_session_lock() {
  local path="$1"
  [[ -n "$path" ]] || return 1
  rm -f "$path"
}

lock_state() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    printf 'missing\n'
    return 0
  fi
  if lock_pid_alive "$path"; then
    printf 'active\n'
    return 0
  fi
  printf 'stale\n'
}

scan_conflicting_lock() {
  local worktree="$1"
  local allow_tool="${2:-}"
  local allow_task="${3:-}"
  local path
  shopt -s nullglob
  for path in "$(lock_root)"/*.env; do
    if ! load_lock_file "$path"; then
      continue
    fi
    if ! lock_pid_alive "$path"; then
      continue
    fi
    if [[ "${LOCK_WORKTREE:-}" != "$worktree" ]]; then
      continue
    fi
    if [[ "${LOCK_TOOL:-}" == "$allow_tool" && "${LOCK_TASK:-}" == "$allow_task" ]]; then
      continue
    fi
    printf '%s\n' "$path"
    return 0
  done
  return 1
}

cleanup_stale_locks() {
  local path
  shopt -s nullglob
  for path in "$(lock_root)"/*.env; do
    clear_stale_lock_file "$path" >/dev/null 2>&1 || true
  done
}

docs_only_file() {
  case "$1" in
    *.md|*.mdx|*.rst|*.adoc|*.txt|docs/*|contracts/*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

session_changed_files() {
  local worktree="$1"
  local base_head="$2"
  git -C "$worktree" diff --name-only --diff-filter=ACMR "$base_head..HEAD"
}

session_is_docs_only() {
  local worktree="$1"
  local base_head="$2"
  local found=1
  local file
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    found=0
    docs_only_file "$file" || return 1
  done < <(session_changed_files "$worktree" "$base_head")
  [[ "$found" == "0" ]]
}

root_lock_state() {
  local status_file
  status_file="$(root_lock_status_path)"
  [[ -f "$status_file" ]] || {
    printf 'unlocked\n'
    return 0
  }
  unset ROOT_LOCK_STATE ROOT_LOCKED_AT
  # shellcheck disable=SC1090
  . "$status_file"
  printf '%s\n' "${ROOT_LOCK_STATE:-unlocked}"
}

root_locked() {
  [[ "$(root_lock_state)" == "locked" ]]
}

write_root_lock_status() {
  local state="$1"
  local status_file
  status_file="$(root_lock_status_path)"
  mkdir -p "$(dirname "$status_file")"
  {
    printf 'ROOT_LOCK_STATE=%q\n' "$state"
    printf 'ROOT_LOCKED_AT=%q\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } >"$status_file"
}
