#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL="com.getfoolish.openfang.vacation-guard"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST_PATH="$PLIST_DIR/$LABEL.plist"
LOG_DIR="$HOME/.openfang"
OUT_PATH="$ROOT/artifacts/vacation-guard/latest.json"
HISTORY_DIR="$ROOT/artifacts/vacation-guard/history"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
REMOTE_HOST="${REMOTE_HOST:-gagan-arora@192.168.40.234}"
INTERVAL="${INTERVAL:-300}"

if [[ -z "$PYTHON_BIN" ]]; then
  echo "python3 not found in PATH" >&2
  exit 1
fi

mkdir -p "$PLIST_DIR" "$LOG_DIR" "$ROOT/artifacts/vacation-guard/history"

cat >"$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$PYTHON_BIN</string>
    <string>$ROOT/scripts/harness/vacation_guard.py</string>
    <string>--api-base</string>
    <string>http://127.0.0.1:50051</string>
    <string>--heartbeat-path</string>
    <string>/api/status</string>
    <string>--remote-host</string>
    <string>$REMOTE_HOST</string>
    <string>--out</string>
    <string>$OUT_PATH</string>
    <string>--history-dir</string>
    <string>$HISTORY_DIR</string>
    <string>--enforce-single-poller</string>
  </array>
  <key>WorkingDirectory</key>
  <string>$ROOT</string>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>$INTERVAL</integer>
  <key>StandardOutPath</key>
  <string>$LOG_DIR/vacation_guard.log</string>
  <key>StandardErrorPath</key>
  <string>$LOG_DIR/vacation_guard.err.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)" "$PLIST_PATH" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH"
launchctl kickstart -k "gui/$(id -u)/$LABEL"

echo "Installed $LABEL"
echo "plist: $PLIST_PATH"
echo "latest artifact: $OUT_PATH"
echo "stdout log: $LOG_DIR/vacation_guard.log"
echo "stderr log: $LOG_DIR/vacation_guard.err.log"
