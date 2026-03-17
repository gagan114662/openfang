# Contract 14: Per-Provider LLM Rate Limiting
**Agent:** Codex
**Branch:** `codex/14-provider-rate-limit`

## Problem
If 50 agents hit Groq simultaneously and Groq returns 429, there's no backoff. Agents just fail. No retry queue, no per-provider throttle, no adaptive waiting.

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Add per-provider rate limiting with exponential backoff to the LLM driver layer.

1. Create a new module: crates/openfang-runtime/src/provider_throttle.rs

2. Define a ProviderThrottle struct that tracks per-provider state:
   - provider_name: String
   - requests_per_minute: u32 (configurable per provider)
   - token_bucket: governor::RateLimiter (or manual implementation)
   - consecutive_429s: AtomicU32
   - backoff_until: AtomicU64 (timestamp in millis — don't send requests until this time)
   - total_throttled: AtomicU64 (metric counter)

3. Create a ProviderThrottleRegistry that holds a DashMap<String, Arc<ProviderThrottle>>:
   - get_or_create(provider_name) → Arc<ProviderThrottle>
   - Lazy initialization — create throttle on first use for that provider

4. Add config for per-provider limits in KernelConfig:
   [providers.groq]
   requests_per_minute = 30

   [providers.openai]
   requests_per_minute = 500

   [providers.anthropic]
   requests_per_minute = 100

   Use a HashMap<String, ProviderLimitConfig> with serde.
   Default: empty map (no throttling unless configured).

5. In the agent loop, BEFORE calling the LLM driver:
   - Acquire a permit from the provider's throttle
   - If the provider is in backoff (backoff_until > now), sleep until backoff_until
   - Log: tracing::info!(provider = %name, wait_ms = wait, "Rate limiting — waiting for provider")

6. AFTER an LLM call, if the response indicates rate limiting (429 or similar):
   - Increment consecutive_429s
   - Calculate backoff: min(2^consecutive_429s * 1000ms, 60_000ms) — exponential backoff, max 60s
   - Set backoff_until = now + backoff_duration
   - Log: tracing::warn!(provider = %name, backoff_ms = ms, "Provider rate limited — backing off")
   - Add Sentry breadcrumb

7. On successful LLM response:
   - Reset consecutive_429s to 0
   - Update backoff_until to 0

8. Add Prometheus metrics:
   - openfang_provider_throttled_total{provider="groq"} (counter)
   - openfang_provider_backoff_seconds{provider="groq"} (gauge — current backoff duration)
   - openfang_provider_429s_total{provider="groq"} (counter)

9. Expose via API: GET /api/providers/status
   Returns JSON array with each provider's:
   - name, requests_per_minute, consecutive_429s, backoff_until, total_throttled

10. Write tests:
    - Test that token bucket blocks when rate exceeded
    - Test that exponential backoff doubles each time: 1s, 2s, 4s, 8s, 16s, 32s, 60s (cap)
    - Test that successful response resets backoff to 0
    - Test that backoff_until prevents requests until time passes

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

# Confirm new module exists
ls crates/openfang-runtime/src/provider_throttle.rs
# Expected: file exists

# Confirm it's wired into the agent loop
grep -rn 'provider_throttle\|ProviderThrottle' crates/openfang-runtime/src/agent_loop.rs
# Expected: acquire/check calls

# Confirm config
grep -rn 'requests_per_minute\|ProviderLimitConfig' crates/
# Expected: config struct + usage

# Confirm Prometheus metrics
grep -rn 'provider_throttled\|provider_backoff\|provider_429' crates/openfang-api/src/routes.rs
# Expected: 3 metric families

# Confirm API endpoint
grep -rn 'providers/status\|provider_status' crates/openfang-api/src/
# Expected: route + handler

# Live test
curl -s http://127.0.0.1:4200/api/providers/status
# Expected: JSON array of providers with throttle state

# Run tests specifically
cargo test provider_throttle -- --nocapture
cargo test backoff -- --nocapture
```
