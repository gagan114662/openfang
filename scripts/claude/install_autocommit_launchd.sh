#!/bin/sh
# Install the periodic auto-commit as a macOS launchd agent.
# Runs every 15 minutes and commits dirty worktrees automatically.
# Works for BOTH Claude and Codex — no session hooks needed.

set -e

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PLIST_NAME="com.openfang.autocommit"
PLIST_PATH="$HOME/Library/LaunchAgents/${PLIST_NAME}.plist"
SCRIPT_PATH="${REPO_ROOT}/scripts/claude/periodic_autocommit.sh"

chmod +x "$SCRIPT_PATH"

cat > "$PLIST_PATH" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${PLIST_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/sh</string>
        <string>${SCRIPT_PATH}</string>
    </array>
    <key>StartInterval</key>
    <integer>900</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/openfang-autocommit.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/openfang-autocommit.log</string>
</dict>
</plist>
PLIST

# Load the agent
launchctl unload "$PLIST_PATH" 2>/dev/null || true
launchctl load "$PLIST_PATH"

echo "Installed: $PLIST_PATH"
echo "Auto-commit runs every 15 minutes for: $REPO_ROOT"
echo "Logs: /tmp/openfang-autocommit.log"
echo ""
echo "To uninstall: launchctl unload $PLIST_PATH && rm $PLIST_PATH"
