#!/usr/bin/env sh
set -eu

mkdir -p "$(dirname "${OFCP_DB_PATH:-/app/data/ofcp.sqlite3}")"

echo '[ofcp] migrating...'
/app/bin/openfang_control_plane eval "OpenfangControlPlane.Release.migrate"

echo '[ofcp] starting...'
exec /app/bin/openfang_control_plane start

