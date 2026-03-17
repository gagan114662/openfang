#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL="com.getfoolish.openfang.autonomous-loop"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST_PATH="$PLIST_DIR/$LABEL.plist"
LOG_DIR="$HOME/.openfang"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
INTERVAL="${INTERVAL:-1800}"

if [[ -z "$PYTHON_BIN" ]]; then
  echo "python3 not found in PATH" >&2
  exit 1
fi

mkdir -p "$PLIST_DIR" "$LOG_DIR" "$ROOT/artifacts/autonomy"

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
    <string>$ROOT/scripts/autonomous_loop.py</string>
    <string>--api-base</string>
    <string>http://127.0.0.1:50051</string>
    <string>--dashboard-base</string>
    <string>http://127.0.0.1:4200</string>
    <string>--sleep-secs</string>
    <string>$INTERVAL</string>
  </array>
  <key>WorkingDirectory</key>
  <string>$ROOT</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$LOG_DIR/autonomous_loop.log</string>
  <key>StandardErrorPath</key>
  <string>$LOG_DIR/autonomous_loop.err.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)" "$PLIST_PATH" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH"
launchctl kickstart -k "gui/$(id -u)/$LABEL"

echo "Installed $LABEL"
echo "plist: $PLIST_PATH"
echo "stdout log: $LOG_DIR/autonomous_loop.log"
echo "stderr log: $LOG_DIR/autonomous_loop.err.log"
