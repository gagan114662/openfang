# Contract 13: Event Bus Backpressure & Metrics
**Agent:** Claude
**Branch:** `claude/13-event-bus-backpressure`

## Problem
`crates/openfang-kernel/src/event_bus.rs` line 27 has:
```rust
let (sender, _) = broadcast::channel(1024);  // Fixed, not configurable
```
And line ~45:
```rust
let _ = sender.send(event);  // Silently drops on overflow!
```
At 100 agents, events get dropped and you'll never know.

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Add backpressure handling, metrics, and configurability to the event bus.

1. Find the event bus in crates/openfang-kernel/src/event_bus.rs

2. Add a config field:
   - event_bus_capacity: usize, default 4096 (increase from 1024)
   - Add #[serde(default)] and Default impl entry

3. Use the config value instead of hardcoded 1024 when creating the broadcast channel

4. Replace EVERY `let _ = sender.send(event)` with proper error handling:
   - When send fails (channel full), increment a drop counter
   - Log: tracing::warn!(dropped_total = counter, "Event bus overflow — event dropped for target {:?}", event.target);
   - Add a Sentry breadcrumb on drop

5. Add an atomic counter for tracking:
   - events_published: AtomicU64 — total events sent
   - events_dropped: AtomicU64 — total events dropped
   - Use std::sync::atomic::Ordering::Relaxed for both

6. Add a method: pub fn stats(&self) -> EventBusStats that returns both counters

7. Expose in Prometheus metrics (/api/metrics):
   - openfang_event_bus_published_total (counter)
   - openfang_event_bus_dropped_total (counter)
   - openfang_event_bus_capacity (gauge)

8. Also expose via a new API endpoint: GET /api/events/stats
   - Returns JSON: { "published": N, "dropped": N, "capacity": N }
   - Register this route in server.rs

9. HISTORY_SIZE (the ring buffer) should also be configurable:
   - event_bus_history_size: usize, default 2000 (increase from 1000)

10. Write tests:
    - Test that events_published increments after publish
    - Test that events_dropped increments when channel is full (create channel with capacity 1, publish 2 events)
    - Test that stats() returns correct counters

When done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. grep -rn 'let _ = .*send' crates/openfang-kernel/src/event_bus.rs returns ZERO results (no more silent drops)

You are NOT done until all four checks pass.
```

## Verification (you run this)

```bash
# The three mandatory checks
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm no silent drops remain
grep -rn 'let _ = .*send' crates/openfang-kernel/src/event_bus.rs
# Expected: nothing

# Confirm counters exist
grep -rn 'events_published\|events_dropped' crates/openfang-kernel/src/event_bus.rs
# Expected: AtomicU64 declarations

# Confirm config fields
grep -rn 'event_bus_capacity\|event_bus_history_size' crates/
# Expected: config + event_bus.rs

# Confirm Prometheus metrics
grep -rn 'event_bus_published\|event_bus_dropped\|event_bus_capacity' crates/openfang-api/src/routes.rs
# Expected: 3 metrics

# Confirm new API endpoint
grep -rn 'events/stats\|event_stats' crates/openfang-api/src/
# Expected: route + handler

# Live test
curl -s http://127.0.0.1:4200/api/events/stats
# Expected: {"published": N, "dropped": 0, "capacity": 4096}

curl -s http://127.0.0.1:4200/api/metrics | grep event_bus
# Expected: 3 metric lines
```
