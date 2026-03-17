# evlog: Wide Event Logging → Sentry + Stdout

**Date:** 2026-03-03
**Status:** Approved

## Problem

Scattered `tracing::info!()` / `tracing::warn!()` calls across routes generate noise.
A single request can produce 10+ log lines with no unified view.
Sentry gets errors but not the full request context (agent, LLM usage, tools).

## Solution

Implement evlog-style **wide events** — one comprehensive structured event per request.
Each event accumulates all context throughout the request lifecycle and emits at the end as:
1. A **Sentry transaction** with child spans + breadcrumbs
2. A **single structured JSON log line** via `tracing::info!()`

## Core API

```rust
// In any route handler:
fn handler(evlog: EvLog, ...) {
    evlog.set("agent", json!({"id": id, "name": name, "provider": "groq"}));
    evlog.set("llm", json!({"tokens_in": 150, "tokens_out": 80}));

    // Structured errors with why/fix
    evlog.error("Payment failed", "Stripe returned card_declined", "Try a different card");

    // Child spans for performance tracking
    let _span = evlog.span("llm_call");
    // ... span auto-finishes on drop
}
```

## Output Format

### Stdout (single JSON line)
```json
{
  "timestamp": "2026-03-03T10:23:45.612Z",
  "level": "info",
  "service": "openfang",
  "method": "POST",
  "path": "/api/agents/abc/message",
  "duration_ms": 1200,
  "status": 200,
  "request_id": "uuid",
  "agent": {"id": "abc", "name": "helper", "provider": "groq", "model": "llama-3.3-70b"},
  "llm": {"tokens_in": 150, "tokens_out": 80, "duration_ms": 950},
  "tools": [{"name": "web_search", "duration_ms": 200, "outcome": "ok"}]
}
```

### Sentry
- Transaction per request (`op: "http.server"`)
- Child spans: `llm_call`, `tool_invoke`, etc.
- All `evlog.set()` data as transaction `extra`
- Errors get full wide-event context via `configure_scope()`
- `why` / `fix` fields on error events

## Files Changed

| File | Change |
|------|--------|
| `crates/openfang-api/src/evlog.rs` | **New** — `EvLog` struct, `set()`, `span()`, `error()` |
| `crates/openfang-api/src/middleware.rs` | Enhance `request_logging()` to create/emit `EvLog` |
| `crates/openfang-api/src/routes.rs` | Key handlers enriched with `evlog.set()` calls |
| `crates/openfang-api/src/lib.rs` | Export `evlog` module |

## Config

```toml
[evlog]
sampling_rate = 1.0
stdout_enabled = true
sentry_enabled = true
```

## Not Included (YAGNI)
- Separate crate (HTTP-layer only, stays in openfang-api)
- Custom persistence (Sentry IS the storage)
- Dashboard UI (Sentry dashboard)
