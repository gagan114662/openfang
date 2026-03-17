#!/usr/bin/env bash
set -euo pipefail

# Invariant checker for OpenFang CI.
# Catches text-level invariants that can't be expressed as #[test].
# Compatible with bash 3.2+ (macOS default).

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$ROOT/crates/openfang-types/src/config.rs"
ROUTES="$ROOT/crates/openfang-api/src/routes.rs"

ERRORS=0
WARNINGS=0

# Helper: PascalCase struct name to TOML channel key.
# Most are simple snake_case but some have known overrides.
struct_to_channel() {
    local raw="$1"
    case "$raw" in
        WhatsAppConfig)   echo "whatsapp" ;;
        DingTalkConfig)   echo "dingtalk" ;;
        RocketChatConfig) echo "rocketchat" ;;
        *)
            echo "$raw" | sed 's/Config$//' | \
                sed 's/\([A-Z]\)/_\1/g' | \
                sed 's/^_//' | \
                tr '[:upper:]' '[:lower:]'
            ;;
    esac
}

# ── Check A: Config type map completeness ──────────────────────────────────
#
# Every non-String field in a channel config struct must have an entry in
# get_config_field_type(). Otherwise the dashboard writes "8443" instead of
# 8443 and TOML deserialization breaks.

echo "=== Check A: Config type map completeness ==="

TMPDIR_INV=$(mktemp -d)
trap 'rm -rf "$TMPDIR_INV"' EXIT

CONFIG_FIELDS="$TMPDIR_INV/config_fields.txt"
TYPE_MAP="$TMPDIR_INV/type_map.txt"

# Channel config struct names — we only check these, not non-channel structs
CHANNEL_STRUCTS="TelegramConfig DiscordConfig SlackConfig WhatsAppConfig SignalConfig \
MatrixConfig EmailConfig TeamsConfig MattermostConfig IrcConfig GoogleChatConfig \
TwitchConfig RocketChatConfig ZulipConfig XmppConfig LineConfig ViberConfig \
MessengerConfig RedditConfig MastodonConfig BlueskyConfig FeishuConfig RevoltConfig \
NextcloudConfig GuildedConfig KeybaseConfig ThreemaConfig NostrConfig WebexConfig \
PumbleConfig FlockConfig TwistConfig MumbleConfig DingTalkConfig DiscourseConfig \
GitterConfig NtfyConfig GotifyConfig WebhookConfig LinkedInConfig"

current_struct=""
current_channel=""
in_channel_struct=0
while IFS= read -r line; do
    # Detect struct declarations
    if echo "$line" | grep -qE 'pub[[:space:]]+struct[[:space:]]+[A-Za-z]+Config[[:space:]]*\{'; then
        raw_name=$(echo "$line" | sed -E 's/.*pub[[:space:]]+struct[[:space:]]+([A-Za-z]+Config).*/\1/')

        if echo "$CHANNEL_STRUCTS" | grep -qw "$raw_name"; then
            current_struct="$raw_name"
            current_channel=$(struct_to_channel "$raw_name")
            in_channel_struct=1
        else
            in_channel_struct=0
            current_struct=""
            current_channel=""
        fi
        continue
    fi

    [ "$in_channel_struct" -eq 0 ] && continue

    # Detect end of struct
    if echo "$line" | grep -qE '^[[:space:]]*\}[[:space:]]*$'; then
        current_struct=""
        current_channel=""
        in_channel_struct=0
        continue
    fi

    # Skip overrides, serde attributes, comments
    echo "$line" | grep -q "overrides" && continue
    echo "$line" | grep -qE '^[[:space:]]*#\[' && continue
    echo "$line" | grep -qE '^[[:space:]]*//' && continue

    # Match non-String fields
    if echo "$line" | grep -qE 'pub[[:space:]]+[a-z_]+:[[:space:]]*(u16|u32|u64|i32|i64|bool|Vec<)'; then
        field=$(echo "$line" | sed -E 's/.*pub[[:space:]]+([a-z_]+):.*/\1/')
        type_raw=$(echo "$line" | sed -E 's/.*pub[[:space:]]+[a-z_]+:[[:space:]]*(.*),?[[:space:]]*$/\1/' | sed 's/,$//')

        expected=""
        case "$type_raw" in
            u16*|u32*|u64*|i32*|i64*) expected="Integer" ;;
            bool*) expected="Boolean" ;;
            *Vec*i64*|*Vec*u64*|*Vec*i32*|*Vec*u32*) expected="IntegerArray" ;;
            *Vec*) expected="StringArray" ;;
        esac

        if [ -n "$expected" ]; then
            echo "${current_channel}.${field} ${expected}" >> "$CONFIG_FIELDS"
        fi
    fi
done < "$CONFIG"

# Extract entries from get_config_field_type() in routes.rs
# Output format: channel.field Type
grep -oE '\("[a-z_]+", *"[a-z_]+"\) *=> *ConfigFieldType::[A-Za-z]+' "$ROUTES" | \
    sed 's/("//;s/", *"/./;s/") *=> *ConfigFieldType::/ /' > "$TYPE_MAP" || true

# Compare: every config field should have a type map entry
missing_count=0
total_count=0
if [ -f "$CONFIG_FIELDS" ]; then
    while IFS= read -r entry; do
        key=$(echo "$entry" | cut -d' ' -f1)
        expected=$(echo "$entry" | cut -d' ' -f2)
        total_count=$((total_count + 1))

        if ! grep -q "^${key} " "$TYPE_MAP" 2>/dev/null; then
            echo "  MISSING: $key (expected $expected)"
            missing_count=$((missing_count + 1))
        fi
    done < "$CONFIG_FIELDS"
fi

if [ "$missing_count" -gt 0 ]; then
    echo "  FAIL: $missing_count non-String config field(s) missing from get_config_field_type()"
    ERRORS=$((ERRORS + 1))
else
    echo "  PASSED: All $total_count non-String config fields have type map entries"
fi

# ── Check B: Route handler registration (warning-only) ────────────────────
#
# Every pub async fn in routes.rs should be referenced in server.rs.
# This is warning-only until the baseline is clean.

echo ""
echo "=== Check B: Route handler registration (warning) ==="

SERVER="$ROOT/crates/openfang-api/src/server.rs"

HANDLERS="$TMPDIR_INV/handlers.txt"
grep -oE 'pub async fn [a-z_]+' "$ROUTES" | sed 's/pub async fn //' | sort -u > "$HANDLERS"

SERVER_REFS="$TMPDIR_INV/server_refs.txt"
grep -oE 'routes::[a-z_]+' "$SERVER" 2>/dev/null | sed 's/routes:://' | sort -u > "$SERVER_REFS" || true

unregistered=0
handler_count=0
while IFS= read -r handler; do
    handler_count=$((handler_count + 1))
    if ! grep -qw "$handler" "$SERVER_REFS" 2>/dev/null; then
        echo "  WARNING: routes::$handler() not referenced in server.rs"
        unregistered=$((unregistered + 1))
        WARNINGS=$((WARNINGS + 1))
    fi
done < "$HANDLERS"

if [ "$unregistered" -eq 0 ]; then
    echo "  PASSED: All $handler_count route handlers are referenced in server.rs"
else
    echo "  $unregistered handler(s) not referenced (warning-only)"
fi

# ── Summary ────────────────────────────────────────────────────────────────

echo ""
echo "=== Summary ==="
echo "  Errors:   $ERRORS"
echo "  Warnings: $WARNINGS"

if [ "$ERRORS" -gt 0 ]; then
    echo "  FAILED"
    exit 1
fi

echo "  PASSED"
exit 0
