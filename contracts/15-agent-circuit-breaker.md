# Contract 15: Agent Circuit Breaker
**Agent:** Claude
**Branch:** `claude/15-circuit-breaker`

## Problem
If an agent fails 100 times in a row, it keeps retrying forever. No pause, no backoff. It just burns tokens and fills Sentry with noise. At 100 agents, a few broken agents can drown out real errors.

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Add a circuit breaker pattern to individual agents that auto-pauses after consecutive failures.

1. Create a new module: crates/openfang-kernel/src/circuit_breaker.rs

2. Define a CircuitBreaker struct per agent:
   - state: CircuitState enum { Closed, Open, HalfOpen }
     - Closed = normal operation (requests flow through)
     - Open = tripped (all requests immediately fail, agent paused)
     - HalfOpen = testing (allow ONE request through to see if it succeeds)
   - consecutive_failures: AtomicU32
   - failure_threshold: u32 (configurable — default 5)
   - recovery_timeout_secs: u64 (configurable — default 60)
   - opened_at: Option<Instant> (when the circuit tripped)
   - total_trips: AtomicU64 (metric counter — how many times this agent has been circuit-broken)
   - last_error: RwLock<Option<String>> (last error message that caused trip)

3. Implement these methods:
   - record_success() → resets consecutive_failures to 0, transitions to Closed
   - record_failure(error: &str) → increments counter, trips to Open if threshold reached
   - allow_request() → returns bool:
     - Closed: true
     - Open: check if recovery_timeout has elapsed → if yes, transition to HalfOpen and return true
     - HalfOpen: true (allow the test request)
   - state() → returns current CircuitState
   - stats() → returns CircuitBreakerStats { state, consecutive_failures, total_trips, last_error }

4. Add config fields:
   - circuit_breaker_failure_threshold: u32, default 5
   - circuit_breaker_recovery_secs: u64, default 60
   Add #[serde(default)] and Default impl entries.

5. Attach a CircuitBreaker to each agent in the agent registry.
   When the kernel creates/spawns an agent, create its circuit breaker.

6. In the agent loop (crates/openfang-runtime/src/agent_loop.rs):
   BEFORE running the loop:
   - Check circuit_breaker.allow_request()
   - If false, return immediately with an error: "Agent circuit breaker is OPEN — paused after {} consecutive failures. Will retry in {}s"
   - Log: tracing::warn!(agent_id = %id, state = ?state, "Circuit breaker blocking agent execution")

   AFTER the loop completes:
   - If success: circuit_breaker.record_success()
   - If error: circuit_breaker.record_failure(&error_message)

7. When circuit trips to Open:
   - Emit a kernel event: CircuitBreakerTripped { agent_id, failures, last_error }
   - Send Sentry event with agent context: sentry::capture_message("Circuit breaker tripped", Level::Warning)
   - Log: tracing::error!(agent_id = %id, failures = count, "Circuit breaker TRIPPED — agent paused")

8. When circuit recovers (HalfOpen → Closed):
   - Emit: CircuitBreakerRecovered { agent_id }
   - Log: tracing::info!(agent_id = %id, "Circuit breaker recovered — agent resumed")

9. Add API endpoint: GET /api/agents/{id}/circuit-breaker
   Returns: { "state": "closed|open|half_open", "consecutive_failures": N, "total_trips": N, "last_error": "...", "recovery_in_secs": N }

10. Add to the list_agents response: include circuit breaker state for each agent

11. Add Prometheus metrics:
    - openfang_circuit_breaker_trips_total{agent_id="..."} (counter)
    - openfang_circuit_breaker_state{agent_id="...",state="open"} (gauge, 1 if open, 0 if not)

12. Write tests:
    - Test: 4 failures → still Closed (threshold is 5)
    - Test: 5 failures → transitions to Open, allow_request() returns false
    - Test: Open + wait recovery_timeout → transitions to HalfOpen, allow_request() returns true
    - Test: HalfOpen + success → transitions to Closed
    - Test: HalfOpen + failure → transitions back to Open
    - Test: record_success resets counter to 0

When done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes

You are NOT done until all three checks pass.
```

## Verification (you run this)

```bash
# The three mandatory checks
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm module exists
ls crates/openfang-kernel/src/circuit_breaker.rs
# Expected: file exists

# Confirm it's declared in kernel mod
grep -rn 'mod circuit_breaker\|pub mod circuit_breaker' crates/openfang-kernel/src/
# Expected: module declaration

# Confirm wired into agent loop
grep -rn 'circuit_breaker\|CircuitBreaker' crates/openfang-runtime/src/agent_loop.rs
# Expected: allow_request + record_success/failure calls

# Confirm wired into agent registry
grep -rn 'circuit_breaker\|CircuitBreaker' crates/openfang-kernel/src/kernel.rs
# Expected: creation on spawn + attachment to agent

# Confirm config fields
grep -rn 'circuit_breaker_failure_threshold\|circuit_breaker_recovery' crates/
# Expected: config struct definition

# Confirm API endpoint
grep -rn 'circuit.breaker\|circuit_breaker' crates/openfang-api/src/routes.rs
# Expected: handler function

# Confirm Prometheus metrics
grep -rn 'circuit_breaker_trips\|circuit_breaker_state' crates/openfang-api/src/routes.rs
# Expected: metric lines

# Run circuit breaker tests specifically
cargo test circuit_breaker -- --nocapture
# Expected: 6+ tests passing

# Live test: start daemon
curl -s http://127.0.0.1:4200/api/agents | python3 -c "
import sys,json
agents = json.load(sys.stdin)
for a in agents[:3]:
    print(f\"{a['id']}: {a.get('circuit_breaker', 'NOT PRESENT')}\")
"
# Expected: circuit breaker state shown per agent

# Check specific agent
curl -s http://127.0.0.1:4200/api/agents/YOUR_AGENT_ID/circuit-breaker
# Expected: {"state": "closed", "consecutive_failures": 0, "total_trips": 0, ...}
```
