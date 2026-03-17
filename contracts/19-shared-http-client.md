# Contract 19 — Share a Single reqwest::Client Across All Utility Functions

## Problem
21 call sites create `reqwest::Client::new()` or `Client::builder().build()` per invocation. Each allocates a fresh connection pool, TLS session cache, and DNS resolver. At 100 agents, this means hundreds of redundant TLS handshakes per minute.

The LLM drivers (anthropic.rs and openai.rs) already do it right — they store `client` in the driver struct. The utility functions don't.

## Exact Locations (per-call client creation)
- `crates/openfang-runtime/src/tts.rs:103` — `reqwest::Client::new()`
- `crates/openfang-runtime/src/tts.rs:175` — `reqwest::Client::new()`
- `crates/openfang-runtime/src/image_gen.rs:33` — `reqwest::Client::new()`
- `crates/openfang-runtime/src/media_understanding.rs:137` — `reqwest::Client::new()`
- `crates/openfang-runtime/src/host_functions.rs:294` — `reqwest::Client::new()`
- `crates/openfang-runtime/src/tool_runner.rs:1453` — `Client::builder()`
- `crates/openfang-runtime/src/tool_runner.rs:1493` — `Client::builder()`
- `crates/openfang-runtime/src/tool_runner.rs:2756` — `Client::builder()`
- `crates/openfang-runtime/src/provider_health.rs:44` — `Client::builder()`
- `crates/openfang-runtime/src/provider_health.rs:156` — `Client::builder()`
- `crates/openfang-runtime/src/web_search.rs:33` — `Client::builder()`
- `crates/openfang-runtime/src/web_fetch.rs:24` — `Client::builder()`
- `crates/openfang-runtime/src/mcp.rs:441` — `Client::builder()`

## Implementation

### Step 1: Create a shared client singleton
In `crates/openfang-runtime/src/lib.rs` or a new `http_client.rs`:
```rust
use once_cell::sync::Lazy;
use std::time::Duration;

pub static SHARED_HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(20)
        .build()
        .expect("Failed to build shared HTTP client")
});
```

### Step 2: Replace per-call creations
For simple cases (`reqwest::Client::new()`), replace with `SHARED_HTTP_CLIENT.clone()` (clone on `Client` is cheap — just an Arc bump).

For builder cases that need custom config (e.g., custom timeout, redirect policy), keep the builder BUT document why it can't use the shared client.

### Step 3: Audit which builders NEED custom config
- `web_fetch.rs` — needs `redirect(Policy::limited(5))` → keep builder, document it
- `mcp.rs` — needs custom timeout → keep builder, document it
- `tool_runner.rs` — needs `danger_accept_invalid_certs` in some cases → keep builder
- Everything else → use shared client

### Step 4: For kept builders, at minimum reuse a shared base builder
```rust
pub fn shared_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(20)
}
```
Then custom sites do: `shared_client_builder().redirect(Policy::limited(5)).build()`

## Done Criteria
- `reqwest::Client::new()` appears zero times outside of struct constructors and the shared singleton
- Per-call `Client::builder()` only exists where custom config is documented with a comment explaining why
- Shared client or shared builder is used everywhere else
- `cargo build --workspace --lib` compiles
- `cargo test --workspace` passes
- `cargo clippy --workspace --all-targets -- -D warnings` zero warnings

## Verification Commands
```bash
# 1. Count remaining Client::new() — should be 0-1 (only the singleton)
grep -rn 'reqwest::Client::new()' crates/openfang-runtime/src/ --include="*.rs"
# EXPECTED: 0 matches (singleton uses builder, not ::new())

# 2. Shared client exists
grep -rn 'SHARED_HTTP_CLIENT\|shared_client_builder' crates/openfang-runtime/src/ --include="*.rs"
# EXPECTED: definition + multiple usage sites

# 3. Remaining builders have justification comments
grep -B2 'Client::builder()' crates/openfang-runtime/src/ -rn --include="*.rs" | grep -i 'custom\|special\|needs\|requires\|cannot use shared'
# EXPECTED: every remaining builder has a comment above it

# 4. tts.rs, image_gen.rs, media_understanding.rs use shared client
grep -n 'SHARED_HTTP_CLIENT\|shared_client' crates/openfang-runtime/src/tts.rs crates/openfang-runtime/src/image_gen.rs crates/openfang-runtime/src/media_understanding.rs
# EXPECTED: at least 1 match per file

# 5. Build + test + clippy
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
