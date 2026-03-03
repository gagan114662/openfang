# OpenFang — Worktree Setup for Claude + Codex

## Step 0: Create a New GitHub Repo (One Time)

```bash
# On GitHub: create new repo "openfang" under your account
# Then from your project directory:

cd ~/path/to/openfang

# Add your new repo as a remote (replace with your actual repo URL)
gh repo create gagan114662/openfang --public --source=. --push

# Or if repo already exists:
git remote add myfork https://github.com/gagan114662/openfang.git
git push myfork main
```

## Step 1: Push Current State to Your New Repo

```bash
# Make sure main is clean
git checkout main
git push myfork main

# Push the CLAUDE.md and verify-last-build.sh
git add CLAUDE.md verify-last-build.sh AGENTS.md contracts/
git commit -m "chore: add agent instructions and verification tooling"
git push myfork main
```

## Step 2: Create Worktrees

```bash
# From your main repo directory
cd ~/path/to/openfang

# Claude worktree — for Claude Code sessions
git worktree add ../openfang-claude -b claude/current-task main

# Codex worktree — for Codex CLI sessions
git worktree add ../openfang-codex -b codex/current-task main

# Verify
git worktree list
```

You now have:
```
~/path/to/openfang/           ← main (you review + merge here)
~/path/to/openfang-claude/    ← Claude works here
~/path/to/openfang-codex/     ← Codex works here
```

## Step 3: Per-Contract Workflow

### Starting a Contract

```bash
# Remove old worktree if it exists
git worktree remove ../openfang-claude --force 2>/dev/null

# Create fresh worktree from latest main
git worktree add ../openfang-claude -b claude/kill-panics main
```

### Opening Claude on the Worktree

```bash
cd ../openfang-claude
claude
# Paste the contract prompt
```

### Opening Codex on the Worktree

```bash
cd ../openfang-codex
codex
# Paste the contract prompt
```

### After the Agent Finishes

```bash
# Run verification from the worktree
cd ../openfang-claude
bash verify-last-build.sh

# If it passes, push the branch
git push myfork claude/kill-panics

# Create PR on GitHub
gh pr create --base main --head claude/kill-panics \
  --title "fix: replace panics with proper error handling in channels" \
  --body "Contract 1: Kill the Panics"

# Review the PR on GitHub, merge it
gh pr merge --squash

# Update main locally
cd ~/path/to/openfang
git pull myfork main

# Clean up worktree
git worktree remove ../openfang-claude
git branch -d claude/kill-panics
```

## Step 4: Running Both in Parallel

```bash
# Terminal 1 — Claude
git worktree remove ../openfang-claude --force 2>/dev/null
git worktree add ../openfang-claude -b claude/kill-panics main
cd ../openfang-claude && claude

# Terminal 2 — Codex
git worktree remove ../openfang-codex --force 2>/dev/null
git worktree add ../openfang-codex -b codex/a2a-typed-errors main
cd ../openfang-codex && codex

# Terminal 3 — You review when they're done
```

### Merging Parallel Work

```bash
# Always merge one at a time, verify after each
cd ~/path/to/openfang

# Merge Claude's work first
git merge claude/kill-panics
bash verify-last-build.sh    # MUST pass

# Then merge Codex's work
git merge codex/a2a-typed-errors
bash verify-last-build.sh    # MUST pass (catches merge conflicts)

# Push to GitHub
git push myfork main
```

## Quick Reference: Worktree Commands

```bash
git worktree list                              # See all worktrees
git worktree add ../name -b branch-name main   # Create new worktree from main
git worktree remove ../name                    # Delete a worktree
git worktree remove ../name --force            # Force delete (dirty worktree)
git worktree prune                             # Clean up stale references
```
