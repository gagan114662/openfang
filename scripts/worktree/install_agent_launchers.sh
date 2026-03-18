#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
BIN_DIR="${OPENFANG_BIN_DIR:-$HOME/.openfang/bin}"
REAL_CLAUDE_BIN="${OPENFANG_REAL_CLAUDE_BIN:-/opt/homebrew/bin/claude}"
REAL_CODEX_BIN="${OPENFANG_REAL_CODEX_BIN:-$(command -v codex || true)}"

if [[ ! -x "$REAL_CLAUDE_BIN" ]]; then
  echo "error: Claude binary not found at $REAL_CLAUDE_BIN" >&2
  exit 1
fi

if [[ -z "$REAL_CODEX_BIN" || ! -x "$REAL_CODEX_BIN" ]]; then
  echo "error: Codex binary not found on PATH" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"

cat >"$BIN_DIR/of-claude" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec bash "$REPO_ROOT/scripts/worktree/run_agent_tool.sh" claude "\$@"
EOF

cat >"$BIN_DIR/of-codex" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec bash "$REPO_ROOT/scripts/worktree/run_agent_tool.sh" codex "\$@"
EOF

cat >"$BIN_DIR/claude" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec bash "$REPO_ROOT/scripts/worktree/agent_entry.sh" claude "$REAL_CLAUDE_BIN" "\$@"
EOF

chmod +x "$BIN_DIR/of-claude" "$BIN_DIR/of-codex" "$BIN_DIR/claude"

echo "Installed OpenFang agent launchers into $BIN_DIR"
