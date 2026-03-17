# Contract 18 — Replace unsafe set_var/remove_var with Thread-Safe Env Store

## Problem
`routes.rs:2300-2302` and `routes.rs:2375-2377` call `std::env::set_var()` and `std::env::remove_var()` inside async Axum handlers. Since Rust 1.66 these are `unsafe` because they mutate process-global state. On a multithreaded Tokio runtime, any concurrent `std::env::var()` read is UB.

## Exact Locations
- `crates/openfang-api/src/routes.rs` ~line 2300: `unsafe { std::env::set_var(env_var, value); }`
- `crates/openfang-api/src/routes.rs` ~line 2375: `unsafe { std::env::remove_var(env_var); }`

## Implementation

### Step 1: Create a thread-safe env overlay
Add to `crates/openfang-kernel/src/` or `crates/openfang-types/src/`:
```rust
use std::collections::HashMap;
use std::sync::RwLock;
use once_cell::sync::Lazy;

static ENV_OVERLAY: Lazy<RwLock<HashMap<String, String>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub fn set_secret_env(key: &str, value: &str) {
    ENV_OVERLAY.write().unwrap().insert(key.to_string(), value.to_string());
}

pub fn remove_secret_env_var(key: &str) {
    ENV_OVERLAY.write().unwrap().remove(key);
}

pub fn get_env_or_overlay(key: &str) -> Option<String> {
    // Check overlay first, then fall back to real env
    if let Some(val) = ENV_OVERLAY.read().unwrap().get(key) {
        return Some(val.clone());
    }
    std::env::var(key).ok()
}
```

### Step 2: Replace all unsafe env ops in routes.rs
```rust
// BEFORE:
unsafe { std::env::set_var(env_var, value); }

// AFTER:
set_secret_env(env_var, value);
```

```rust
// BEFORE:
unsafe { std::env::remove_var(env_var); }

// AFTER:
remove_secret_env_var(env_var);
```

### Step 3: Update all env var reads for API keys
Anywhere the codebase does `std::env::var("GROQ_API_KEY")` etc., replace with `get_env_or_overlay("GROQ_API_KEY")`. This ensures runtime-configured keys are visible to LLM drivers.

### Step 4: Remove ALL `unsafe` blocks from routes.rs
After this change, zero `unsafe` blocks should remain.

## Done Criteria
- Zero `std::env::set_var` or `std::env::remove_var` in routes.rs
- Zero `unsafe` blocks in routes.rs
- New env overlay module exists with `set_secret_env`, `remove_secret_env_var`, `get_env_or_overlay`
- All LLM drivers can still read API keys (both from real env and overlay)
- `cargo build --workspace --lib` compiles
- `cargo test --workspace` passes
- `cargo clippy --workspace --all-targets -- -D warnings` zero warnings

## Verification Commands
```bash
# 1. No more std::env::set_var in routes
grep -rn 'std::env::set_var\|std::env::remove_var' crates/openfang-api/src/routes.rs
# EXPECTED: zero matches

# 2. No unsafe blocks in routes.rs
grep -n 'unsafe {' crates/openfang-api/src/routes.rs
# EXPECTED: zero matches

# 3. Env overlay module exists
grep -rn 'fn set_secret_env\|fn get_env_or_overlay\|fn remove_secret_env' crates/ --include="*.rs"
# EXPECTED: at least 3 matches (the function definitions)

# 4. Overlay is actually used
grep -rn 'set_secret_env\|get_env_or_overlay' crates/openfang-api/src/routes.rs
# EXPECTED: matches where the old unsafe calls were

# 5. Build + test + clippy
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
