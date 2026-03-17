#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="${OPENFANG_CONFIG:-$HOME/.openfang/config.toml}"
ORG="${SENTRY_ORG:-foolish}"
PROJECT="${SENTRY_PROJECT:-openfang-monitoring}"
INCLUDE_SOURCES="${INCLUDE_SOURCES:-true}"
WAIT_SECS="${SENTRY_UPLOAD_WAIT_SECS:-30}"

usage() {
  cat <<'EOF'
Usage: scripts/upload_sentry_debug_files.sh [PATH ...]

Uploads native debug files for OpenFang binaries to Sentry.

Env overrides:
  SENTRY_AUTH_TOKEN          Explicit auth token.
  SENTRY_ORG                 Defaults to "foolish".
  SENTRY_PROJECT             Defaults to "openfang-monitoring".
  OPENFANG_CONFIG            Config file to read sentry.auth_token from.
  INCLUDE_SOURCES            "true" (default) or "false".
  SENTRY_UPLOAD_WAIT_SECS    Server processing wait time, default 30.

If no PATH is provided, the script uploads:
  target/debug/openfang
  target/release/openfang
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

load_auth_token() {
  if [[ -n "${SENTRY_AUTH_TOKEN:-}" ]]; then
    printf '%s\n' "$SENTRY_AUTH_TOKEN"
    return
  fi

  if [[ -f "$CONFIG_PATH" ]]; then
    python3 - "$CONFIG_PATH" <<'PY'
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore

path = Path(sys.argv[1])
try:
    payload = tomllib.loads(path.read_text(encoding="utf-8"))
except Exception:
    print("")
    raise SystemExit(0)

sentry_cfg = payload.get("sentry")
if isinstance(sentry_cfg, dict):
    token = str(sentry_cfg.get("auth_token") or "").strip()
    print(token)
else:
    print("")
PY
    return
  fi

  printf '\n'
}

AUTH_TOKEN="$(load_auth_token)"
if [[ -z "$AUTH_TOKEN" ]]; then
  echo "Missing Sentry auth token. Set SENTRY_AUTH_TOKEN or sentry.auth_token in $CONFIG_PATH." >&2
  exit 1
fi

declare -a CANDIDATES=()
if [[ "$#" -gt 0 ]]; then
  for path in "$@"; do
    CANDIDATES+=("$path")
  done
else
  CANDIDATES+=(
    "$ROOT_DIR/target/debug/openfang"
    "$ROOT_DIR/target/release/openfang"
  )
fi

declare -a EXISTING=()
for path in "${CANDIDATES[@]}"; do
  if [[ -e "$path" ]]; then
    EXISTING+=("$path")
  fi
done

if [[ "${#EXISTING[@]}" -eq 0 ]]; then
  echo "No debug file candidates found." >&2
  exit 1
fi

CMD=(
  sentry-cli
  debug-files
  upload
  --auth-token "$AUTH_TOKEN"
  --org "$ORG"
  --project "$PROJECT"
  --wait-for "$WAIT_SECS"
)

if [[ "$INCLUDE_SOURCES" == "true" ]]; then
  CMD+=(--include-sources)
fi

CMD+=("${EXISTING[@]}")

echo "Uploading debug files to Sentry org=$ORG project=$PROJECT"
for path in "${EXISTING[@]}"; do
  echo "  - $path"
done

"${CMD[@]}"
