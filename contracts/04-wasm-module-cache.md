# Contract 04: WASM Module Caching
**Agent:** Codex
**Branch:** `codex/04-wasm-cache`

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Add in-memory LRU caching for compiled WASM modules in the sandbox.

Currently in crates/openfang-runtime/src/sandbox.rs, WASM modules are recompiled from bytes on every invocation. This is slow.

1. Add the `lru` crate to openfang-runtime/Cargo.toml (or use DashMap with eviction)
2. Create a module-level cache keyed by SHA-256 hash of the WASM bytes
3. The cache should store compiled wasmtime::Module objects
4. Cache flow: hash the WASM bytes → check cache → if hit, clone the Module → if miss, compile and insert
5. Add a config field wasm_module_cache_size to KernelConfig in crates/openfang-types/ or crates/openfang-kernel/src/config.rs with #[serde(default)] defaulting to 64
6. Make sure to add the default value in the Default impl too
7. The cache should be wrapped in Arc<Mutex<LruCache>> or similar for thread safety
8. Write a test that:
   - Compiles the same WASM bytes twice
   - Asserts the cache has 1 entry (not 2)
   - If possible, asserts second compilation is faster

When you're done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. grep -rn 'wasm_module_cache' shows the config field exists

You are NOT done until all four checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test wasm_cache -- --nocapture
grep -rn 'wasm_module_cache' crates/
```
