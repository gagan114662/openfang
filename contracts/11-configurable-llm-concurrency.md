# Contract 11: Configurable LLM Concurrency
**Agent:** Claude
**Branch:** `claude/11-llm-concurrency`

## Problem
`crates/openfang-kernel/src/background.rs` line 17 has:
```rust
const MAX_CONCURRENT_BG_LLM: usize = 5;
```
This is hardcoded. With 100 agents, 95 sit waiting. The semaphore also only applies to background agents — foreground agents have no limit at all.

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Make the LLM concurrency limit configurable and apply it to ALL agent types.

1. In crates/openfang-kernel/src/background.rs, find the hardcoded constant:
   const MAX_CONCURRENT_BG_LLM: usize = 5;

2. Add a config field to KernelConfig (in openfang-types/src/config.rs or openfang-kernel/src/config.rs):
   - Field name: max_concurrent_llm_calls
   - Type: usize
   - Default: 50
   - Add #[serde(default = "default_max_concurrent_llm")]
   - Add to Default impl
   - Add doc comment: "Maximum number of concurrent LLM API calls across all agents"

3. Remove the hardcoded const. Pass the config value into BackgroundExecutor::new()

4. ALSO create a GLOBAL semaphore (not just background) that is shared between:
   - BackgroundExecutor (for autonomous/scheduled agents)
   - The foreground agent_loop (for user-triggered messages)
   - Store it on the Kernel struct as: pub llm_semaphore: Arc<tokio::sync::Semaphore>

5. In the agent loop (crates/openfang-runtime/src/agent_loop.rs), acquire the global semaphore before calling the LLM driver. Release it after the response. Use a guard pattern so it auto-releases on error.

6. Add a Prometheus metric to the /api/metrics endpoint:
   - openfang_llm_semaphore_available (gauge) — available permits
   - openfang_llm_semaphore_waiters (gauge) — number of tasks waiting (if possible, otherwise skip)

7. Add a Sentry breadcrumb when a task waits more than 5 seconds for a permit:
   tracing::warn!(wait_secs = elapsed, "LLM semaphore contention — agent waited {}s for permit", elapsed);

8. Write tests:
   - Test that config field deserializes correctly with default of 50
   - Test that semaphore with capacity 2 blocks the 3rd concurrent call
   - Test that semaphore releases on error (drop guard)

When done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. grep -rn 'MAX_CONCURRENT_BG_LLM' returns ZERO results (hardcoded const removed)
5. grep -rn 'max_concurrent_llm' shows config field exists

You are NOT done until all five checks pass.
```

## Verification (you run this)

```bash
# The three mandatory checks
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm hardcoded const is gone
grep -rn 'MAX_CONCURRENT_BG_LLM' crates/
# Expected: nothing

# Confirm config field exists
grep -rn 'max_concurrent_llm' crates/
# Expected: config.rs definition + background.rs usage + agent_loop.rs usage

# Confirm semaphore is on Kernel struct
grep -rn 'llm_semaphore' crates/openfang-kernel/src/kernel.rs
# Expected: field declaration + initialization

# Confirm Prometheus metric exists
grep -rn 'llm_semaphore_available\|llm_semaphore_waiters' crates/openfang-api/src/routes.rs
# Expected: at least 1 match

# Live test: start daemon, check metrics
curl -s http://127.0.0.1:4200/api/metrics | grep llm_semaphore
# Expected: openfang_llm_semaphore_available 50
```
