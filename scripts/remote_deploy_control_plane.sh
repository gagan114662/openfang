#!/usr/bin/env bash
set -euo pipefail

# Remote-first deploy:
# - rsync this repo to the remote host
# - build OpenFang (Rust) on remote
# - install the primary daemon as a systemd --user service
# - install the unattended guard as a systemd --user timer
# - keep the legacy control plane disabled to preserve single Telegram ownership
#
# Requirements:
# - SSH key access to REMOTE
# - remote user can run docker without sudo
#
# Usage:
#   REMOTE=gagan-arora@192.168.40.234 ./scripts/remote_deploy_control_plane.sh

REMOTE="${REMOTE:-gagan-arora@192.168.40.234}"
# IMPORTANT: This runs on your Mac. Keep REMOTE_DIR relative by default so rsync
# targets the remote user's HOME (e.g. "open_fang" => "~/open_fang").
REMOTE_DIR="${REMOTE_DIR:-open_fang}"
COPY_LOCAL_OPENFANG_CONFIG="${COPY_LOCAL_OPENFANG_CONFIG:-1}"
TMP_RUNTIME_ENV="$(mktemp)"
TMP_CONFIG_OVERLAY="$(mktemp)"
if [[ -f "$HOME/.openfang/secrets.env" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$HOME/.openfang/secrets.env"
  set +a
fi
cleanup() {
  rm -f "$TMP_RUNTIME_ENV"
  rm -f "$TMP_CONFIG_OVERLAY"
}
trap cleanup EXIT

for var_name in \
  OPENAI_API_KEY \
  ANTHROPIC_API_KEY \
  GROQ_API_KEY \
  TAVILY_API_KEY \
  BRAVE_API_KEY \
  PERPLEXITY_API_KEY \
  OPENFANG_WEBHOOK_TOKEN \
  EMAIL_PASSWORD
do
  if [[ -n "${!var_name:-}" ]]; then
    printf '%s=%q\n' "$var_name" "${!var_name}" >> "$TMP_RUNTIME_ENV"
  fi
done

if [[ -f "$HOME/.openfang/config.toml" ]]; then
  OPENFANG_ADMIN_TELEGRAM_CHAT_ID="$(
    python3 - <<'PY'
from pathlib import Path
import sys
try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

cfg = tomllib.loads((Path.home() / ".openfang" / "config.toml").read_text())
telegram = ((cfg.get("channels") or {}).get("telegram") or {})
chat_id = telegram.get("admin_chat_id")
if chat_id is None:
    allowed = telegram.get("allowed_users") or []
    if allowed:
        chat_id = allowed[0]
if chat_id is not None:
    print(chat_id)
PY
  )"
  if [[ -n "${OPENFANG_ADMIN_TELEGRAM_CHAT_ID:-}" ]]; then
    printf '%s=%q\n' "OPENFANG_ADMIN_TELEGRAM_CHAT_ID" "$OPENFANG_ADMIN_TELEGRAM_CHAT_ID" >> "$TMP_RUNTIME_ENV"
  fi
  python3 - <<'PY' > "$TMP_CONFIG_OVERLAY"
from pathlib import Path

source = Path.home() / ".openfang" / "config.toml"
text = source.read_text()
wanted = {"channels.telegram", "sentry"}
current = None
blocks = []
lines = []
for raw in text.splitlines():
    stripped = raw.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        if current in wanted and lines:
            blocks.append("\n".join(lines).strip() + "\n")
        current = stripped[1:-1]
        lines = [raw]
    elif current in wanted:
        lines.append(raw)
if current in wanted and lines:
    blocks.append("\n".join(lines).strip() + "\n")
print("\n".join(blocks).strip())
PY
fi

echo "[deploy] syncing repo to $REMOTE:$REMOTE_DIR ..."
rsync -az --delete \
  --exclude 'target/' \
  --exclude '**/node_modules/' \
  --exclude '.git/' \
  --exclude '.DS_Store' \
  --exclude '.mcp_data/' \
  --exclude 'crates/**/dist/' \
  "./" "$REMOTE:$REMOTE_DIR/"

ssh "$REMOTE" "mkdir -p ~/.openfang"
if [[ -s "$TMP_RUNTIME_ENV" ]]; then
  echo "[deploy] syncing runtime env snapshot to remote ~/.openfang/runtime.env ..."
  rsync -az "$TMP_RUNTIME_ENV" "$REMOTE:.openfang/runtime.env"
fi
if [[ "$COPY_LOCAL_OPENFANG_CONFIG" != "0" && -s "$TMP_CONFIG_OVERLAY" ]]; then
  echo "[deploy] syncing Telegram/Sentry config overlay to remote ~/.openfang/config.overlay.toml ..."
  rsync -az "$TMP_CONFIG_OVERLAY" "$REMOTE:.openfang/config.overlay.toml"
fi

echo "[deploy] running remote install/build ..."
ssh -t "$REMOTE" "REMOTE_DIR='$REMOTE_DIR' bash -s" <<'EOS'
set -euo pipefail

REMOTE_DIR="${REMOTE_DIR:-open_fang}"
case "$REMOTE_DIR" in
  /*) ;;
  *) REMOTE_DIR="$HOME/$REMOTE_DIR" ;;
esac
export PATH="$HOME/.cargo/bin:$PATH"

echo "[remote] repo at: $REMOTE_DIR"
cd "$REMOTE_DIR"

echo "[remote] building OpenFang (release, low parallelism) ..."
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --release -p openfang-cli

mkdir -p "$HOME/.openfang"
mkdir -p "$HOME/.openfang/codex_homes/gagan" "$HOME/.openfang/codex_homes/vandan"
mkdir -p "$HOME/.openfang/browser/profiles/sentry-primary" "$HOME/.openfang/browser/profiles/sentry-fallback"
if [ ! -f "$HOME/.openfang/config.toml" ]; then
cat > "$HOME/.openfang/config.toml" <<'EOF'
log_level = "info"
api_listen = "127.0.0.1:50051"
api_key = ""

[ws]
enabled = true
default_codec = "json"
max_frame_bytes = 2097152
ping_interval_ms = 20000
idle_timeout_ms = 60000
resume_ttl_ms = 120000
default_credits = 32

[default_model]
provider = "codex-cli"
model = "codex-5.3"
api_key_env = ""

[exec_policy]
mode = "allowlist"
# Minimal host allowlist for CI-style health sweeps and repo work. Expand as needed.
allowed_commands = ["bash","sh","cargo","rustc","git","rg","sed","awk","python3","node","npm","make"]
timeout_secs = 2700
max_output_bytes = 1048576

[browser]
headless = true
user_data_dir = "/home/gagan-arora/.openfang/browser/profiles/sentry-primary"
cookie_backup_interval_secs = 300

[autonomy]
enabled = true
primary_host = "gpu"
primary_sentry_browser_host = "gpu"
fallback_sentry_browser_host = "mac"
ops_agent_name = "ops-coder"
ops_agent_unlimited_quota = true
guard_trace_enabled = true
guard_interval_secs = 15
scheduled_idle_grace_secs = 300
heartbeat_dedupe_secs = 1800
telegram_admin_alert_cooldown_secs = 1800
autofix_enabled = true
deploy_enabled = true
allow_patch_validate_deploy = true
max_autofix_attempts_per_issue = 3
max_deploys_per_day = 8
fallback_browser_required = true
EOF
else
  echo "[remote] preserving existing ~/.openfang/config.toml"
fi

if [ -s "$HOME/.openfang/config.overlay.toml" ]; then
  python3 - <<'PY'
from pathlib import Path

cfg_path = Path.home() / ".openfang" / "config.toml"
overlay_path = Path.home() / ".openfang" / "config.overlay.toml"
cfg_text = cfg_path.read_text() if cfg_path.exists() else ""
overlay_text = overlay_path.read_text()

def sections(text: str):
    current = None
    lines = []
    out = {}
    for raw in text.splitlines():
        stripped = raw.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            if current and lines:
                out[current] = "\n".join(lines).strip() + "\n"
            current = stripped[1:-1]
            lines = [raw]
        elif current:
            lines.append(raw)
    if current and lines:
        out[current] = "\n".join(lines).strip() + "\n"
    return out

cfg_sections = sections(cfg_text)
overlay_sections = sections(overlay_text)
merged = cfg_text.rstrip()
for name, block in overlay_sections.items():
    if name in cfg_sections:
        continue
    merged += ("\n\n" if merged else "") + block.strip()
cfg_path.write_text(merged.rstrip() + "\n")
PY
fi

python3 - <<'PY'
from pathlib import Path

cfg_path = Path.home() / ".openfang" / "config.toml"
text = cfg_path.read_text()

browser_block = """
[browser]
headless = true
user_data_dir = "{home}/.openfang/browser/profiles/sentry-primary"
cookie_backup_interval_secs = 300
""".strip()

autonomy_block = """
[autonomy]
enabled = true
primary_host = "gpu"
primary_sentry_browser_host = "gpu"
fallback_sentry_browser_host = "mac"
ops_agent_name = "ops-coder"
ops_agent_unlimited_quota = true
guard_trace_enabled = true
guard_interval_secs = 15
scheduled_idle_grace_secs = 300
heartbeat_dedupe_secs = 1800
telegram_admin_alert_cooldown_secs = 1800
autofix_enabled = true
deploy_enabled = true
allow_patch_validate_deploy = true
max_autofix_attempts_per_issue = 3
max_deploys_per_day = 8
fallback_browser_required = true
""".strip()

updated = text.rstrip()
if "[browser]" not in text:
    updated += "\n\n" + browser_block.format(home=str(Path.home()))
if "[autonomy]" not in text:
    updated += "\n\n" + autonomy_block
cfg_path.write_text(updated.rstrip() + "\n")
PY

mkdir -p "$HOME/.config/systemd/user"

cat > "$HOME/.config/systemd/user/openfang.service" <<EOF
[Unit]
Description=OpenFang daemon (Rust)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$REMOTE_DIR
Environment=PATH=%h/.cargo/bin:%h/.npm-global/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
EnvironmentFile=-%h/.openfang/runtime.env
# Two-account Codex OAuth failover:
# Put gagan@getfoolish.com creds under %h/.openfang/codex_homes/gagan
# Put vandan@getfoolish.com creds under %h/.openfang/codex_homes/vandan
Environment=OPENFANG_CODEX_HOME_PRIMARY=%h/.openfang/codex_homes/gagan
Environment=OPENFANG_CODEX_HOME_SECONDARY=%h/.openfang/codex_homes/vandan
Environment=OPENFANG_LLM_SUBPROCESS_TIMEOUT_SECS=900
Environment=OPENFANG_PRIMARY_HOST=gpu
Environment=OPENFANG_TELEGRAM_OWNER=mac
Environment=OPENFANG_PRIMARY_SENTRY_BROWSER_HOST=gpu
Environment=OPENFANG_FALLBACK_SENTRY_BROWSER_HOST=mac
Environment=OPENFANG_TELEGRAM_ALERT_COOLDOWN_SECS=1800
ExecStart=$REMOTE_DIR/target/release/openfang start
Restart=always
RestartSec=2
TimeoutStopSec=10
KillSignal=SIGINT
Environment=OPENFANG_HOST_ROLE=primary

[Install]
WantedBy=default.target
EOF

cat > "$HOME/.config/systemd/user/openfang-vacation-guard.service" <<EOF
[Unit]
Description=OpenFang unattended guard
After=openfang.service
Wants=openfang.service

[Service]
Type=oneshot
WorkingDirectory=$REMOTE_DIR
Environment=PATH=%h/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
Environment=OPENFANG_HOST_ROLE=primary
Environment=OPENFANG_PRIMARY_HOST=gpu
Environment=OPENFANG_TELEGRAM_OWNER=mac
Environment=OPENFANG_PRIMARY_SENTRY_BROWSER_HOST=gpu
Environment=OPENFANG_FALLBACK_SENTRY_BROWSER_HOST=mac
Environment=OPENFANG_TELEGRAM_ALERT_COOLDOWN_SECS=1800
ExecStart=/usr/bin/env python3 $REMOTE_DIR/scripts/harness/vacation_guard.py --api-base http://127.0.0.1:50051 --remote-host local --enforce-single-poller
EOF

cat > "$HOME/.config/systemd/user/openfang-vacation-guard.timer" <<'EOF'
[Unit]
Description=Run OpenFang unattended guard every 15 seconds

[Timer]
OnBootSec=20
OnUnitActiveSec=15
AccuracySec=1s
Unit=openfang-vacation-guard.service

[Install]
WantedBy=timers.target
EOF

echo "[remote] systemd: enabling openfang ..."
systemctl --user daemon-reload
systemctl --user enable openfang.service
systemctl --user reset-failed openfang.service >/dev/null 2>&1 || true
systemctl --user restart openfang.service
systemctl --user enable --now openfang-vacation-guard.timer

echo "[remote] waiting for OpenFang health ..."
for i in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:50051/api/health >/dev/null 2>&1; then
    echo "[remote] OpenFang up"
    break
  fi
  sleep 1
done

systemctl --user reset-failed openfang-vacation-guard.service >/dev/null 2>&1 || true
systemctl --user start openfang-vacation-guard.service || true
curl -sS http://127.0.0.1:50051/api/health || true

echo "[remote] seeding unattended workloads ..."
python3 "$REMOTE_DIR/scripts/seed_unattended_workloads.py" \
  --api-base http://127.0.0.1:50051 \
  --ops-agent-manifest "$REMOTE_DIR/config/agents/ops-coder.toml" || true

echo "[remote] disabling legacy control plane to preserve single Telegram poller ..."
systemctl --user disable --now openfang_control_plane.service >/dev/null 2>&1 || true

echo ""
echo "[remote] DONE. To watch logs:"
echo "  journalctl --user -u openfang -f"
echo "  journalctl --user -u openfang-vacation-guard.service -f"
echo ""
echo "[remote] IMPORTANT: the Rust daemon is now the primary Telegram owner."
EOS

echo ""
echo "[deploy] From your Mac, to view live logs:"
echo "  ssh $REMOTE 'journalctl --user -u openfang -f'"
echo "  ssh $REMOTE 'journalctl --user -u openfang-vacation-guard.service -f'"
echo ""
echo "[deploy] To use the dashboard without running OpenFang locally:"
echo "  ssh -L 50051:127.0.0.1:50051 $REMOTE"
echo "  open http://127.0.0.1:50051/"
