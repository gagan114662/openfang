# Contract 01: Kill the Panics
**Agent:** Claude
**Branch:** `claude/01-kill-panics`

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Replace every panic!() in the channel bridge files with proper error handling.

Grep the entire crates/openfang-channels/src/ directory for panic!("Expected and panic!("expected. There are 30+ instances across Discord, Slack, Telegram, Reddit, Bluesky, Mastodon, and other bridge files.

For each panic:
- If it's inside a match arm, replace with returning an Err() or logging the error with tracing::error!() and using `continue` to skip that message
- If it's inside a function that returns Result, propagate the error with ?
- If the function doesn't return Result, log with tracing::error!() and return early
- Do NOT change any public function signatures

When you're done, verify:
1. `grep -rn 'panic!("Expected\|panic!("expected' crates/openfang-channels/src/` returns ZERO results
2. cargo build --workspace --lib passes
3. cargo test --workspace passes
4. cargo clippy --workspace --all-targets -- -D warnings passes with zero warnings

You are NOT done until all four checks pass. Do not edit tests to make them pass.
```

## Verification (you run this after)

```bash
# Automated checks
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm panics are gone
grep -rn 'panic!("Expected\|panic!("expected' crates/openfang-channels/src/
# Expected output: nothing

# Count remaining panics anywhere (informational)
grep -rn 'panic!' crates/openfang-channels/src/ | wc -l
# Should be 0 or near-zero
```
