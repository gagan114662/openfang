#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# OpenFang: Merge a worktree branch back to main
# Usage: bash merge-and-verify.sh claude/01-kill-panics
# ============================================================

if [ -z "${1:-}" ]; then
  echo "Usage: bash merge-and-verify.sh <branch-name>"
  echo ""
  echo "Available branches:"
  git branch | grep -E 'claude/|codex/' | sed 's/^/  /'
  exit 1
fi

BRANCH="$1"
REMOTE=$(git remote | head -1)

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

echo ""
echo "========================================="
echo "  Merging: $BRANCH → main"
echo "========================================="
echo ""

# Switch to main
git checkout main
git pull "$REMOTE" main 2>/dev/null || true

# Merge the branch
echo ">> Merging $BRANCH..."
if git merge "$BRANCH" --no-edit; then
  echo -e "${GREEN}Merge succeeded${NC}"
else
  echo -e "${RED}Merge conflict! Resolve manually, then re-run this script.${NC}"
  exit 1
fi

# Run verification
echo ""
echo ">> Running verification..."
echo ""

PASS=true

echo ">> cargo build --workspace --lib"
if cargo build --workspace --lib 2>&1; then
  echo -e "${GREEN}[PASS] Build${NC}"
else
  echo -e "${RED}[FAIL] Build${NC}"
  PASS=false
fi

echo ""
echo ">> cargo test --workspace"
if cargo test --workspace 2>&1; then
  echo -e "${GREEN}[PASS] Tests${NC}"
else
  echo -e "${RED}[FAIL] Tests${NC}"
  PASS=false
fi

echo ""
echo ">> cargo clippy --workspace --all-targets -- -D warnings"
if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
  echo -e "${GREEN}[PASS] Clippy${NC}"
else
  echo -e "${RED}[FAIL] Clippy${NC}"
  PASS=false
fi

echo ""
echo "========================================="

if [ "$PASS" = true ]; then
  echo -e "${GREEN}ALL CHECKS PASSED${NC}"
  echo ""
  read -p "Push to remote and clean up? (y/N) " -n 1 -r
  echo
  if [[ $REPLY =~ ^[Yy]$ ]]; then
    git push "$REMOTE" main
    echo ">> Pushed main to $REMOTE"

    # Clean up worktree and branch
    WORKTREE_DIR=$(git worktree list | grep "$BRANCH" | awk '{print $1}')
    if [ -n "$WORKTREE_DIR" ]; then
      git worktree remove "$WORKTREE_DIR" --force 2>/dev/null || true
      echo ">> Removed worktree: $WORKTREE_DIR"
    fi
    git branch -d "$BRANCH" 2>/dev/null || true
    echo ">> Deleted branch: $BRANCH"
    echo ""
    echo -e "${GREEN}DONE! Ready for next contract.${NC}"
  fi
else
  echo -e "${RED}VERIFICATION FAILED — rolling back merge${NC}"
  git reset --hard HEAD~1
  echo "Rolled back to previous main. Fix the issues in the worktree and re-merge."
fi
