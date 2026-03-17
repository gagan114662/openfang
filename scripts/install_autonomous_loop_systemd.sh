#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT_PATH="$UNIT_DIR/openfang-autonomous-loop.service"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"

if [[ -z "$PYTHON_BIN" ]]; then
  echo "python3 not found in PATH" >&2
  exit 1
fi

mkdir -p "$UNIT_DIR" "$HOME/.openfang" "$ROOT/artifacts/autonomy"

cat >"$UNIT_PATH" <<EOF
[Unit]
Description=OpenFang Autonomous Loop
After=network-online.target openfang.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$ROOT
ExecStart=$PYTHON_BIN $ROOT/scripts/autonomous_loop.py --api-base http://127.0.0.1:50051 --dashboard-base http://127.0.0.1:4200
Environment=OPENFANG_AUTONOMY_ENABLED=true
Restart=always
RestartSec=60
StandardOutput=append:%h/.openfang/autonomous_loop.log
StandardError=append:%h/.openfang/autonomous_loop.err.log

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now openfang-autonomous-loop.service

echo "Installed openfang-autonomous-loop.service"
echo "unit: $UNIT_PATH"
echo "stdout log: $HOME/.openfang/autonomous_loop.log"
echo "stderr log: $HOME/.openfang/autonomous_loop.err.log"
