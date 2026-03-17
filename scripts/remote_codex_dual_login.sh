#!/usr/bin/env bash
set -euo pipefail

# Bootstraps TWO Codex CLI OAuth sessions on the remote host by isolating them
# into two separate HOME directories. OpenFang can then fail over between them
# via OPENFANG_CODEX_HOME_PRIMARY/SECONDARY.
#
# Why HOME isolation?
# - Codex CLI stores OAuth state under $HOME/.codex
# - Codex CLI does not expose a "profile" flag for multiple accounts
#
# Usage:
#   REMOTE=gagan-arora@192.168.40.234 ./scripts/remote_codex_dual_login.sh
#
# After running, you must complete the device-code flow in your browser twice.

REMOTE="${REMOTE:-gagan-arora@192.168.40.234}"

# IMPORTANT: these defaults must be valid on the REMOTE host, not your Mac.
# We pass them as relative paths and resolve them to absolute paths on the remote.
PRIMARY_HOME_REMOTE="${PRIMARY_HOME_REMOTE:-.openfang/codex_homes/gagan}"
SECONDARY_HOME_REMOTE="${SECONDARY_HOME_REMOTE:-.openfang/codex_homes/vandan}"

echo "[codex] remote: $REMOTE"
echo "[codex] primary HOME:   $PRIMARY_HOME_REMOTE"
echo "[codex] secondary HOME: $SECONDARY_HOME_REMOTE"
echo ""

ssh -tt "$REMOTE" "PRIMARY_HOME_REMOTE='$PRIMARY_HOME_REMOTE' SECONDARY_HOME_REMOTE='$SECONDARY_HOME_REMOTE' bash -s" <<'EOS'
set -euo pipefail

PRIMARY_HOME_REMOTE="${PRIMARY_HOME_REMOTE:?}"
SECONDARY_HOME_REMOTE="${SECONDARY_HOME_REMOTE:?}"

export PATH="$HOME/.npm-global/bin:$HOME/.cargo/bin:$PATH"

resolve_home() {
  local p="$1"
  case "$p" in
    /*) printf "%s" "$p" ;;
    *) printf "%s" "$HOME/$p" ;;
  esac
}

PRIMARY_HOME_REMOTE="$(resolve_home "$PRIMARY_HOME_REMOTE")"
SECONDARY_HOME_REMOTE="$(resolve_home "$SECONDARY_HOME_REMOTE")"

if ! command -v codex >/dev/null 2>&1; then
  echo "[codex] ERROR: codex not found on PATH. Install it first:"
  echo "  npm config set prefix \"\$HOME/.npm-global\""
  echo "  npm i -g @openai/codex"
  exit 1
fi

mkdir -p "$PRIMARY_HOME_REMOTE" "$SECONDARY_HOME_REMOTE"

echo "[codex] codex version: $(codex --version)"
echo ""

echo "[codex] PRIMARY login (this should be gagan@getfoolish.com)"
echo "        A device-code + URL will appear. Complete it in your browser."
HOME="$PRIMARY_HOME_REMOTE" codex login --device-auth
echo ""
echo "[codex] PRIMARY status:"
HOME="$PRIMARY_HOME_REMOTE" codex login status || true
echo ""

echo "[codex] SECONDARY login (this should be vandan@getfoolish.com)"
echo "        A device-code + URL will appear. Complete it in your browser."
HOME="$SECONDARY_HOME_REMOTE" codex login --device-auth
echo ""
echo "[codex] SECONDARY status:"
HOME="$SECONDARY_HOME_REMOTE" codex login status || true
echo ""

echo "[codex] DONE. Next step: wire OpenFang service env vars:"
echo "  OPENFANG_CODEX_HOME_PRIMARY=$PRIMARY_HOME_REMOTE"
echo "  OPENFANG_CODEX_HOME_SECONDARY=$SECONDARY_HOME_REMOTE"
EOS
