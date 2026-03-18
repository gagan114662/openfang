#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYMPHONY_ELIXIR_ROOT="${SYMPHONY_ELIXIR_ROOT:-/Users/gaganarora/Desktop/my projects/symphony/symphony/elixir}"
WORKFLOW_PATH="${OPENFANG_SYMPHONY_WORKFLOW_PATH:-$REPO_ROOT/WORKFLOW.md}"
LOGS_ROOT="${SYMPHONY_LOGS_ROOT:-$REPO_ROOT/artifacts/symphony/log}"
PORT="${SYMPHONY_PORT:-4100}"
ACK_FLAG="--i-understand-that-this-will-be-running-without-the-usual-guardrails"

load_env_file_if_unset() {
  local env_file="$1"
  [[ -f "$env_file" ]] || return 0

  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "${line//[[:space:]]/}" ]] && continue
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    [[ "$line" == *"="* ]] || continue

    local key="${line%%=*}"
    local value="${line#*=}"

    key="${key#"${key%%[![:space:]]*}"}"
    key="${key%"${key##*[![:space:]]}"}"

    [[ -n "$key" ]] || continue
    [[ -n "${!key+x}" ]] && continue

    export "$key=$value"
  done < "$env_file"
}

load_config_value_if_unset() {
  local section="$1"
  local config_key="$2"
  local env_key="$3"
  local config_file="$4"
  [[ -f "$config_file" ]] || return 0
  [[ -n "${!env_key+x}" ]] && return 0

  local value
  value="$(
    awk -F= -v target_section="$section" -v target_key="$config_key" '
      function trim(value) {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
        return value
      }

      /^\[[^]]+\][[:space:]]*$/ {
        current_section = substr($0, 2, length($0) - 2)
        next
      }

      current_section == target_section && $1 ~ "^[[:space:]]*" target_key "[[:space:]]*$" {
        value = trim(substr($0, index($0, "=") + 1))
        sub(/^"/, "", value)
        sub(/"$/, "", value)
        print value
        exit
      }
    ' "$config_file"
  )"

  [[ -n "$value" ]] && export "$env_key=$value"
}

require_bin() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: required binary '$1' is not on PATH" >&2
    exit 1
  }
}

require_bin git
require_bin codex

load_env_file_if_unset "$HOME/.openfang/secrets.env"
load_env_file_if_unset "$HOME/.openfang/.env"
load_config_value_if_unset "sentry" "dsn" "SENTRY_DSN" "$HOME/.openfang/config.toml"
load_config_value_if_unset "sentry" "environment" "SENTRY_ENVIRONMENT" "$HOME/.openfang/config.toml"

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
