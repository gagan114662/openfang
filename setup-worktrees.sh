#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# OpenFang Worktree Setup Script
# Creates parallel worktrees for Claude and Codex
# ============================================================

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
CLAUDE_DIR="${REPO_ROOT}/../openfang-claude"
CODEX_DIR="${REPO_ROOT}/../openfang-codex"

echo ""
echo "========================================="
echo "  OpenFang Worktree Setup"
echo "========================================="
echo ""

# Check we're in a git repo
if ! git -C "$REPO_ROOT" rev-parse --git-dir > /dev/null 2>&1; then
  echo "ERROR: Not a git repo. Run this from the openfang project root."
  exit 1
fi

cd "$REPO_ROOT"

# ---- Step 1: Ensure we're on main and it's clean ----
echo ">> Checking current branch..."
CURRENT_BRANCH=$(git branch --show-current)
echo "   Currently on: $CURRENT_BRANCH"

if [ -n "$(git status --porcelain)" ]; then
  echo ""
  echo "WARNING: You have uncommitted changes."
  echo "Commit or stash them before creating worktrees."
  echo ""
  read -p "Continue anyway? (y/N) " -n 1 -r
  echo
  if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    exit 1
  fi
fi

# ---- Step 2: Check if GitHub remote exists ----
echo ""
echo ">> Checking remotes..."
git remote -v
echo ""

if ! git remote | grep -q "origin\|myfork\|fork"; then
  echo "No remote found. Let's add one."
  echo ""
  read -p "Enter your GitHub repo URL (e.g., https://github.com/you/openfang.git): " REMOTE_URL
  git remote add myfork "$REMOTE_URL"
  echo "Added remote 'myfork'"
fi

# ---- Step 3: Push main to remote ----
echo ""
echo ">> Pushing main to remote..."
REMOTE_NAME=$(git remote | head -1)
git push "$REMOTE_NAME" main 2>/dev/null || echo "   (push failed or already up to date — that's fine)"

# ---- Step 4: Select contract ----
echo ""
echo "========================================="
echo "  Available Contracts"
echo "========================================="
echo ""

CONTRACTS=(contracts/*.md)
for i in "${!CONTRACTS[@]}"; do
  NAME=$(basename "${CONTRACTS[$i]}" .md)
  echo "  [$i] $NAME"
done

echo ""
read -p "Select contract number for CLAUDE (or 'skip'): " CLAUDE_PICK
read -p "Select contract number for CODEX  (or 'skip'): " CODEX_PICK

# ---- Step 5: Create Claude worktree ----
if [ "$CLAUDE_PICK" != "skip" ]; then
  CONTRACT_NAME=$(basename "${CONTRACTS[$CLAUDE_PICK]}" .md)
  BRANCH_NAME="claude/${CONTRACT_NAME}"

  echo ""
  echo ">> Setting up Claude worktree..."
  echo "   Branch: $BRANCH_NAME"
  echo "   Directory: $CLAUDE_DIR"

  # Clean up old worktree if exists
  git worktree remove "$CLAUDE_DIR" --force 2>/dev/null || true
  git branch -D "$BRANCH_NAME" 2>/dev/null || true

  # Create fresh worktree from main
  git worktree add "$CLAUDE_DIR" -b "$BRANCH_NAME" main

  echo ""
  echo "   Claude worktree ready!"
  echo "   To start: cd $CLAUDE_DIR && claude"
  echo "   Contract: ${CONTRACTS[$CLAUDE_PICK]}"
fi

# ---- Step 6: Create Codex worktree ----
if [ "$CODEX_PICK" != "skip" ]; then
  CONTRACT_NAME=$(basename "${CONTRACTS[$CODEX_PICK]}" .md)
  BRANCH_NAME="codex/${CONTRACT_NAME}"

  echo ""
  echo ">> Setting up Codex worktree..."
  echo "   Branch: $BRANCH_NAME"
  echo "   Directory: $CODEX_DIR"

  # Clean up old worktree if exists
  git worktree remove "$CODEX_DIR" --force 2>/dev/null || true
  git branch -D "$BRANCH_NAME" 2>/dev/null || true

  # Create fresh worktree from main
  git worktree add "$CODEX_DIR" -b "$BRANCH_NAME" main

  echo ""
  echo "   Codex worktree ready!"
  echo "   To start: cd $CODEX_DIR && codex"
  echo "   Contract: ${CONTRACTS[$CODEX_PICK]}"
fi

# ---- Step 7: Summary ----
echo ""
echo "========================================="
echo "  READY TO GO"
echo "========================================="
echo ""
echo "  Worktrees:"
git worktree list
echo ""
echo "  Next steps:"
echo "  1. Open Terminal 1:  cd $CLAUDE_DIR && claude"
echo "     Paste the prompt from the contract file"
echo ""
echo "  2. Open Terminal 2:  cd $CODEX_DIR && codex"
echo "     Paste the prompt from the contract file"
echo ""
echo "  3. When done, run:  bash verify-last-build.sh"
echo "     from EACH worktree directory"
echo ""
echo "  4. If verified, push and create PRs:"
echo "     git push $REMOTE_NAME <branch-name>"
echo "     gh pr create --base main --head <branch-name>"
echo ""
echo "  5. After merging PRs, clean up:"
echo "     git worktree remove ../openfang-claude"
echo "     git worktree remove ../openfang-codex"
echo ""
