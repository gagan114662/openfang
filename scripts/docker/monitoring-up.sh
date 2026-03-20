#!/usr/bin/env bash
# Start the OpenFang monitoring stack (Loki + Promtail + Grafana).
# Passes all arguments through to docker compose (e.g. -d, --build).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

docker compose \
  -f "$REPO_ROOT/docker-compose.yml" \
  -f "$REPO_ROOT/docker-compose.monitoring.yml" \
  up "$@"
