# Shared Telegram Bot Integration: OpenFang + Raindrop

**Date:** February 26, 2026
**Status:** Approved
**Author:** Claude Sonnet 4.5

## Context

This design integrates OpenFang and Raindrop through a shared Telegram bot (@OpenClawAIDemoBot). The goal is unified communication: commands control OpenFang agents, incidents from Raindrop get delivered, all in the same Telegram chat.

### Why This Integration

**Problem:** Operating two separate bots creates fragmentation. Users want one interface for both agent control (OpenFang) and observability alerts (Raindrop).

**Goals:**
- Single bot for both systems (@OpenClawAIDemoBot)
- Commands route to OpenFang agents
- Raindrop incidents flow to Telegram
- Both systems work independently if the other is down

**Approach:** OpenFang hosts the bot, subscribes to Raindrop's incident bus.

## Architecture Overview

```
┌─────────────────────────────────────────┐
│         OpenFang (Bot Host)             │
│                                         │
│  ┌─────────────────────────────────┐   │
│  │  TelegramBot (teloxide)         │   │
│  │  - Long polling                 │   │
│  │  - Command parsing              │   │
│  │  - Message sending              │   │
│  └──────┬──────────────────┬───────┘   │
│         │                  │           │
│    Commands           Notifications    │
│         │                  │           │
│         ▼                  ▲           │
│  ┌─────────────┐    ┌─────────────┐   │
│  │   Agent     │    │  Raindrop   │   │
│  │  Execution  │    │  Incident   │   │
│  │             │    │  Subscriber │   │
│  └─────────────┘    └─────────────┘   │
│                            ▲           │
└────────────────────────────┼───────────┘
                             │
                    ┌────────┴────────┐
                    │   Raindrop      │
                    │   Incident Bus  │
                    │   (HTTP SSE)    │
                    └─────────────────┘
```

**OpenFang** becomes the bot host: runs teloxide polling, receives commands, executes agents, subscribes to Raindrop incidents, sends notifications.

**Raindrop** stays independent: publishes incidents to bus, removes direct Telegram sender, no dependency on OpenFang.

## Components

### OpenFang Components (New/Modified)

#### 1. Complete `openfang-telegram` crate

**Replace stub with real implementation:**

```rust
pub struct TelegramBot {
    bot: teloxide::Bot,
    config: TelegramConfig,
}

impl TelegramBot {
    pub async fn start_polling(
        &self,
        command_tx: mpsc::Sender<(String, TelegramCommand)>,
    ) -> Result<(), String>;

    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<(), String>;
}
```

**Methods:**
- `start_polling()` - Run teloxide long-polling loop, parse commands, send to kernel
- `send_message()` - Send text to Telegram chat (for responses and incidents)

#### 2. Raindrop Incident Subscriber

**New module:** `crates/openfang-kernel/src/raindrop_subscriber.rs`

```rust
pub struct RaindropSubscriber {
    raindrop_url: String,
    telegram_bot: Arc<openfang_telegram::TelegramBot>,
    workspace_chat_mapping: HashMap<String, String>, // workspace_id → chat_id
}

impl RaindropSubscriber {
    pub async fn subscribe_and_forward(&self) -> Result<(), String>;
}
```

**Subscribes to:** `GET {raindrop_url}/v1/incidents/stream` (Server-Sent Events)
**Forwards to:** Telegram via `telegram_bot.send_message()`

#### 3. Kernel Integration

**Modified:** `crates/openfang-kernel/src/kernel.rs`

- Start Telegram bot as background task
- Start Raindrop subscriber as background task
- Wire command receiver to agent execution

### Raindrop Components (Modified)

#### 1. Add Incident Stream Endpoint

**New endpoint:** `GET /v1/incidents/stream`

Returns Server-Sent Events stream:
```
event: incident
data: {"id":"123","workspace_id":"ws1","severity":"High",...}

event: incident
data: {"id":"124","workspace_id":"ws1","severity":"Critical",...}
```

#### 2. Keep Telegram Config (But Don't Send)

- `NotificationChannelKind::Telegram` stays in types
- Config stores chat_id, policies
- But `rd-notifier` skips Telegram sending (log "delegated to OpenFang")

## Data Flow

### Incoming: Commands → Agents

1. User sends: `/run researcher analyze Bitcoin` to @OpenClawAIDemoBot
2. Telegram API → OpenFang bot (long polling receives update)
3. Parse command: `TelegramCommand::Run { agent: "researcher", task: "analyze Bitcoin" }`
4. Send to kernel via `command_tx` channel
5. Kernel finds/creates agent "researcher"
6. Send message "analyze Bitcoin" to agent
7. Agent executes → returns response
8. OpenFang sends response to same `chat_id` via Telegram

### Outgoing: Raindrop Incidents → Telegram

1. Raindrop detector creates incident (workspace=ws1, agent=agent-x, severity=High)
2. Published to incident bus
3. Streamed via SSE to OpenFang subscriber
4. OpenFang looks up: `workspace_chat_mapping["ws1"]` → `chat_id`
5. Formats: `"[incident:123] workspace=ws1 agent=agent-x severity=High message=..."`
6. Sends to `chat_id` via Telegram bot

## Configuration

### OpenFang Config (`~/.openfang/config.toml`)

```toml
[channels.telegram]
bot_token_env = "TELEGRAM_BOT_TOKEN"
allowed_users = ["8444910202"]  # Your Telegram user ID
poll_interval_secs = 1

[raindrop]
api_url = "http://localhost:4201"  # Raindrop API
workspace_chat_mapping = { "ws1" = "8444910202", "ws2" = "other_chat" }
```

### Raindrop Config

**No changes needed** - existing Telegram channel config stays for policy/routing logic, but OpenFang handles actual sending.

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Raindrop down | OpenFang bot works for commands, incident subscription retries every 30s |
| OpenFang down | Raindrop incidents buffer in bus, commands fail (no bot) |
| Telegram API down | Queue messages locally, retry with exponential backoff (1s→60s max) |
| Invalid command | Send help text to user |
| Unauthorized user | Silent ignore (no response) |
| Rate limit hit | Queue messages, drain at 20 msg/sec (Telegram limit) |

## Testing

**Unit Tests:**
- Command parsing (already exists ✅)
- Incident formatting from Raindrop types
- Policy filtering logic

**Integration Tests:**
- Mock Telegram update → verify command execution
- Mock Raindrop incident → verify Telegram send

**Live Validation:**
1. Start OpenFang with `TELEGRAM_BOT_TOKEN`
2. Send `/agents` → should list OpenFang agents
3. Send `/run test-agent hello` → should execute and respond
4. Trigger Raindrop incident → should appear in Telegram within 5s
5. Verify unauthorized user gets no response

## Success Criteria

- ✅ Bot responds to commands within 2 seconds
- ✅ Raindrop incidents delivered within 5 seconds
- ✅ No duplicate notifications (Raindrop dedup works)
- ✅ Unauthorized users blocked
- ✅ Both systems operate independently if other is down
- ✅ Rate limiting prevents API exhaustion

---

**Design complete.** Ready to write implementation plan.
