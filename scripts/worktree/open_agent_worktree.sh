#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export OPENFANG_POLICY_REPO_ROOT="${OPENFANG_POLICY_REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd -P)}"
# shellcheck source=./common.sh
. "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: scripts/worktree/open_agent_worktree.sh <claude|codex> <task-name> [base-ref]

Creates or reopens a dedicated linked git worktree for the requested tool.

Examples:
  scripts/worktree/open_agent_worktree.sh claude sentry-triage
  scripts/worktree/open_agent_worktree.sh codex ws-v1 main
EOF
}

if [[ $# -lt 2 || $# -gt 3 ]]; then
  usage >&2
  exit 1
fi

TOOL="$1"
RAW_TASK="$2"
BASE_REF="${3:-main}"

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
BRANCH="$(expected_branch "$TOOL" "$TASK_SLUG")"

mkdir -p "$(dirname "$DEST")"
git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1 || true

if [[ -e "$DEST/.git" || -f "$DEST/.git" ]]; then
  echo "existing worktree"
  echo "tool:    $TOOL"
  echo "branch:  $BRANCH"
  echo "path:    $DEST"
  echo "next:    cd '$DEST'"
  exit 0
fi

if git -C "$REPO_ROOT" rev-parse --verify --quiet "refs/heads/$BRANCH" >/dev/null; then
  git -C "$REPO_ROOT" worktree add "$DEST" "$BRANCH"
else
  git -C "$REPO_ROOT" rev-parse --verify --quiet "$BASE_REF" >/dev/null || {
    echo "error: base ref '$BASE_REF' does not exist" >&2
    exit 1
  }
  git -C "$REPO_ROOT" worktree add -b "$BRANCH" "$DEST" "$BASE_REF"
fi

echo "created worktree"
echo "tool:    $TOOL"
echo "branch:  $BRANCH"
echo "path:    $DEST"
echo "next:    cd '$DEST'"
