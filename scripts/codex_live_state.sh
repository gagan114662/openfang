#!/usr/bin/env bash
set -euo pipefail

REMOTE_HOST="${REMOTE_HOST:-gagan-arora@192.168.40.234}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD_LATEST_PATH="$HOME/.openfang/artifacts/vacation-guard/latest.json"
LOCAL_PID="$(pgrep -f 'target/debug/openfang start|./target/debug/openfang start|openfang start' | head -n 1 || true)"

echo "== Local =="
echo "repo: $REPO_ROOT"
echo "time: $(date '+%Y-%m-%d %H:%M:%S %Z')"
echo "pid: ${LOCAL_PID:-none}"

if [[ -n "${LOCAL_PID}" ]]; then
  echo "-- local sockets --"
  lsof -Pan -p "$LOCAL_PID" -i || true
fi

echo "-- local health --"
curl -fsS http://127.0.0.1:50051/api/health || echo "local api unavailable"
echo

echo "-- local status --"
curl -fsS http://127.0.0.1:50051/api/status | head -c 400 || echo "local status unavailable"
echo

echo "-- local autonomy state --"
curl -fsS http://127.0.0.1:50051/api/autonomy/state | head -c 1200 || echo "local autonomy state unavailable"
echo

echo "-- local daemon log tail --"
tail -n 40 "$HOME/.openfang/daemon.log" 2>/dev/null || true

echo
echo "== Remote =="
ssh -o BatchMode=yes -o ConnectTimeout=5 "$REMOTE_HOST" '
  echo "host: $(hostname)"
  echo "-- remote services --"
  systemctl --user --no-pager --plain status openfang.service openfang_control_plane.service openfang-vacation-guard.timer openfang-vacation-guard.service || true
  echo "-- remote established telegram sockets --"
  ss -tpn | grep -E "149\.154|91\.108" || true
  echo "-- remote latest guard artifact --"
  tail -n 80 "'"$GUARD_LATEST_PATH"'" 2>/dev/null || true
  echo "-- remote latest autonomy state --"
  tail -n 120 "$HOME"/open_fang/artifacts/autonomy/current-state.json 2>/dev/null || true
  echo "-- remote unattended workload registry --"
  sed -n "1,200p" "$HOME"/open_fang/config/unattended_workloads.toml 2>/dev/null || true
'
