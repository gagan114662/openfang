# Contract 17 — Replace Unsafe Config Pointer Mutation with Arc<RwLock>

## Problem
`routes.rs:4576-4593` casts `&KernelConfig` (shared ref behind Arc) to `*mut KernelConfig` and writes through it. Two concurrent PUT `/api/budget` requests cause undefined behavior (torn f64 reads/writes). Same pattern in `kernel.rs:3508`.

## Exact Locations
- `crates/openfang-api/src/routes.rs` lines 4576-4593 (`update_budget`)
- `crates/openfang-kernel/src/kernel.rs` line 3508 (`self_ptr = Arc::as_ptr(self) as *mut`)

## Implementation

### Step 1: Wrap KernelConfig in Arc<RwLock>
In `crates/openfang-kernel/src/kernel.rs`, change the config field:
```rust
// BEFORE:
pub config: KernelConfig,

// AFTER:
pub config: Arc<tokio::sync::RwLock<KernelConfig>>,
```

### Step 2: Update all config reads
Every `self.config.field` or `state.kernel.config.field` becomes:
```rust
let config = state.kernel.config.read().await;
config.field
```

### Step 3: Replace the unsafe mutation in update_budget
```rust
// BEFORE (routes.rs:4576-4593):
let config_ptr = &state.kernel.config as *const KernelConfig as *mut KernelConfig;
unsafe { (*config_ptr).budget.max_hourly_usd = v; }

// AFTER:
let mut config = state.kernel.config.write().await;
if let Some(v) = body["max_hourly_usd"].as_f64() {
    config.budget.max_hourly_usd = v;
}
if let Some(v) = body["max_daily_usd"].as_f64() {
    config.budget.max_daily_usd = v;
}
if let Some(v) = body["max_monthly_usd"].as_f64() {
    config.budget.max_monthly_usd = v;
}
if let Some(v) = body["alert_threshold"].as_f64() {
    config.budget.alert_threshold = v.clamp(0.0, 1.0);
}
```

### Step 4: Fix kernel.rs:3508
Replace `Arc::as_ptr(self) as *mut OpenFangKernel` with proper interior mutability — either `RwLock` or pass `&mut self` if the call site allows it.

### Step 5: Remove ALL `unsafe` blocks in routes.rs
After migration, zero `unsafe` blocks should remain in `routes.rs`.

## Done Criteria
- Zero `as *mut` casts in routes.rs and kernel.rs (except FFI if any)
- Zero `unsafe` blocks in routes.rs
- `cargo build --workspace --lib` compiles
- `cargo test --workspace` passes
- `cargo clippy --workspace --all-targets -- -D warnings` zero warnings

## Verification Commands
```bash
# 1. No more unsafe pointer casts in API routes
grep -n 'as \*mut' crates/openfang-api/src/routes.rs
# EXPECTED: zero matches

# 2. No unsafe blocks in routes.rs
grep -n 'unsafe {' crates/openfang-api/src/routes.rs
# EXPECTED: zero matches

# 3. No pointer casts in kernel.rs
grep -n 'as \*mut' crates/openfang-kernel/src/kernel.rs
# EXPECTED: zero matches

# 4. Config is behind RwLock
grep -n 'RwLock<KernelConfig>' crates/openfang-kernel/src/kernel.rs
# EXPECTED: at least 1 match

# 5. Build + test + clippy
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
