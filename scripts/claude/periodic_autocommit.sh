#!/bin/sh
# Periodic auto-commit for OpenFang repo.
# Runs every 15 minutes via launchd to catch dirty worktrees
# left by Codex or any other tool that doesn't have session hooks.
#
# Install: scripts/claude/install_autocommit_launchd.sh

REPO="/Users/gaganarora/Desktop/my projects/open_fang"

# Only run if repo exists and has changes
if [ ! -d "$REPO/.git" ]; then
    exit 0
fi

cd "$REPO" || exit 0

# Check if dirty
STATUS=$(git status --porcelain 2>/dev/null)
if [ -z "$STATUS" ]; then
    exit 0  # Clean — nothing to do
fi

# Auto-commit
python3 scripts/claude/auto_commit.py "$REPO" 2>/dev/null

# Also emit a telemetry event so you can see it in Sentry
BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
DAEMON_URL=$(python3 -c "
import json, pathlib
p = pathlib.Path.home() / '.openfang' / 'daemon.json'
if p.exists():
    d = json.loads(p.read_text())
    a = d.get('listen_addr','')
    print(f'http://{a}' if a and not a.startswith('http') else a)
else:
    print('http://127.0.0.1:50051')
" 2>/dev/null)

curl -s -X POST "$DAEMON_URL/api/telemetry/structured" \
  -H "Content-Type: application/json" \
  -d "{
    \"body\": \"git.auto_commit.periodic\",
    \"level\": \"info\",
    \"attributes\": {
      \"event.kind\": \"git.auto_commit.periodic\",
      \"git.branch\": \"$BRANCH\",
      \"outcome\": \"success\"
    }
  }" >/dev/null 2>&1
