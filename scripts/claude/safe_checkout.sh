#!/bin/sh
# Safe checkout: auto-commits dirty work before switching branches.
# Usage: ./scripts/claude/safe_checkout.sh <branch-name>
#
# This prevents the "dirty worktree" problem by saving your work
# before switching to another branch.

set -e
BRANCH="$1"

if [ -z "$BRANCH" ]; then
    echo "Usage: safe_checkout.sh <branch-name>"
    exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"

# Check if there are uncommitted changes
if [ -n "$(git status --porcelain)" ]; then
    echo ">> Dirty worktree detected on $CURRENT_BRANCH"
    echo ">> Auto-committing before switching..."
    python3 "$REPO_ROOT/scripts/claude/auto_commit.py" "$REPO_ROOT"
fi

# Now switch
git checkout "$BRANCH"
echo ">> Switched to $BRANCH (previous work on $CURRENT_BRANCH was auto-committed)"
