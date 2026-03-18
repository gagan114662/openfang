#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYMPHONY_ELIXIR_ROOT="${SYMPHONY_ELIXIR_ROOT:-/Users/gaganarora/Desktop/my projects/symphony/symphony/elixir}"
WORKFLOW_PATH="${OPENFANG_SYMPHONY_WORKFLOW_PATH:-$REPO_ROOT/WORKFLOW.md}"
LOGS_ROOT="${SYMPHONY_LOGS_ROOT:-$REPO_ROOT/artifacts/symphony/log}"
PORT="${SYMPHONY_PORT:-4100}"
ACK_FLAG="--i-understand-that-this-will-be-running-without-the-usual-guardrails"

require_bin() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: required binary '$1' is not on PATH" >&2
    exit 1
  }
}

require_bin git
require_bin codex

if [[ -z "${LINEAR_API_KEY:-}" ]]; then
  echo "error: LINEAR_API_KEY is not set" >&2
  exit 1
fi

if [[ ! -d "$SYMPHONY_ELIXIR_ROOT" ]]; then
  echo "error: Symphony checkout not found at $SYMPHONY_ELIXIR_ROOT" >&2
  exit 1
fi

if [[ ! -f "$WORKFLOW_PATH" ]]; then
  echo "error: workflow file not found at $WORKFLOW_PATH" >&2
  exit 1
fi

mkdir -p "$LOGS_ROOT"

cd "$SYMPHONY_ELIXIR_ROOT"

if command -v mise >/dev/null 2>&1; then
  mise trust >/dev/null 2>&1 || true
  if [[ ! -d deps ]]; then
    mise exec -- mix setup
  else
    mise exec -- mix build
  fi
  exec mise exec -- ./bin/symphony "$ACK_FLAG" "$WORKFLOW_PATH" --logs-root "$LOGS_ROOT" --port "$PORT"
fi

if [[ ! -x ./bin/symphony ]]; then
  echo "error: ./bin/symphony is missing and mise is unavailable" >&2
  exit 1
fi

exec ./bin/symphony "$ACK_FLAG" "$WORKFLOW_PATH" --logs-root "$LOGS_ROOT" --port "$PORT"
