# Contract 02: A2A Typed Errors
**Agent:** Codex
**Branch:** `codex/02-a2a-typed-errors`

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Replace all Result<T, String> in the A2A client code with a proper typed error enum.

1. Find all A2A-related files: grep for "a2a" in crates/openfang-runtime/src/ and crates/openfang-types/src/
2. Create an A2aError enum using thiserror with these variants:
   - Network(String) — for HTTP/connection failures
   - Parse(String) — for JSON/deserialization failures
   - Timeout — for request timeouts
   - NotFound(String) — for agent/task not found
   - Protocol(String) — for A2A protocol violations
   - Internal(String) — catch-all
3. Replace every Result<T, String> in the A2A code with Result<T, A2aError>
4. Update all .map_err() calls to use the correct A2aError variant
5. If A2aError needs to be in openfang-types for cross-crate use, put it there

When you're done, verify:
1. grep -rn 'Result<.*String>' on A2A files returns ZERO matches for return types
2. cargo build --workspace --lib passes
3. cargo test --workspace passes
4. cargo clippy --workspace --all-targets -- -D warnings passes

You are NOT done until all four checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm no string errors in A2A code
grep -rn 'Result<.*String>' crates/openfang-runtime/src/a2a*.rs
# Expected: nothing (or only non-return-type matches)
```
