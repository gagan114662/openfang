#!/usr/bin/env bash
set -euo pipefail

# End-to-end Codex smoke test against the REMOTE OpenFang daemon (Rust) via SSH.
# This keeps your Mac light: it only sends an HTTP request and prints the result.
#
# Usage:
#   REMOTE=gagan-arora@192.168.40.234 ./scripts/remote_smoke_codex.sh
#
# Optional:
#   AGENT_NAME=hello-world
#   MESSAGE='...'

REMOTE="${REMOTE:-gagan-arora@192.168.40.234}"
AGENT_NAME="${AGENT_NAME:-hello-world}"
# NOTE: avoid backticks here (they trigger local shell command substitution).
MESSAGE="${MESSAGE:-Remote Codex smoke test: Use tools to run 'uname -a' and 'ls -la ~/open_fang | head', then summarize in 3 bullets.}"

ssh "$REMOTE" "AGENT_NAME='$AGENT_NAME' MESSAGE='$MESSAGE' bash -s" <<'EOS'
set -euo pipefail

API="http://127.0.0.1:50051"

agent_id="$(curl -fsS "$API/api/agents" | python3 -c 'import json,sys,os; agents=json.load(sys.stdin); name=os.environ["AGENT_NAME"]; hits=[a for a in agents if a.get("name")==name or a.get("id")==name]; print(hits[0]["id"] if hits else "")')"

if [ -z "$agent_id" ]; then
  echo "[smoke] ERROR: agent not found: $AGENT_NAME"
  echo "[smoke] Available agents:"
  curl -fsS "$API/api/agents" | python3 -c 'import json,sys; agents=json.load(sys.stdin); [print("-",a.get("name"),a.get("id"),a.get("model_provider"),a.get("model_name")) for a in agents]'
  exit 2
fi

echo "[smoke] agent_id=$agent_id name=$AGENT_NAME"

resp="$(curl -sS -w "\n__HTTP_STATUS__:%{http_code}\n" -X POST "$API/api/agents/$agent_id/message" \
  -H "content-type: application/json" \
  -d "{\"message\": $(python3 -c 'import json,os; print(json.dumps(os.environ[\"MESSAGE\"]))') }")"

status="$(printf "%s" "$resp" | sed -n 's/^__HTTP_STATUS__://p' | tail -1)"
body="$(printf "%s" "$resp" | sed '/^__HTTP_STATUS__:/d')"

echo "[smoke] status=$status"
echo "$body" | head -200

echo ""
echo "[smoke] last codex wide events (if any):"
tail -n 200 ~/.openfang/daemon.log 2>/dev/null | grep -F 'event=\"llm.codex_cli\"' | tail -5 || true
EOS
