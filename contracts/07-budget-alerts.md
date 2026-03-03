# Contract 07: Budget Soft Alerts
**Agent:** Claude
**Branch:** `claude/07-budget-alerts`

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Add soft warning thresholds to the budget system.

Currently the budget system in the kernel only does hard stops when limits are hit. There are no warnings before you hit the wall.

1. Find the budget/metering code — it's in crates/openfang-kernel/src/ (likely metering.rs or budget-related module)
2. Add a config field warning_threshold_percent with #[serde(default)] defaulting to 80 (meaning 80%)
3. Add the default to the Default impl
4. When an agent's cumulative spend crosses the threshold percentage of its limit:
   - Emit a warning event on the kernel event bus (use whatever event pattern already exists)
   - Log with tracing::warn!("Agent {} has used {}% of its budget", agent_id, percentage)
5. In the API responses for GET /api/budget and GET /api/budget/agents/{id}:
   - Add a "warning" boolean field (true if over threshold)
   - Add a "usage_percent" float field
6. Write a unit test that simulates spending to 81% and asserts:
   - The warning flag is true
   - usage_percent is approximately 81.0
7. Write a unit test that simulates spending to 50% and asserts warning is false

When you're done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes

You are NOT done until all three checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test budget_warn -- --nocapture

# Live test after daemon start:
curl -s http://127.0.0.1:4200/api/budget | python3 -m json.tool
# Should show "warning" and "usage_percent" fields
```
