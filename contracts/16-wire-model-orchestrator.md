# Contract 16: Wire Up Model Orchestrator
**Agent:** Codex
**Branch:** `codex/16-model-orchestrator`

## Problem
`crates/openfang-runtime/src/drivers/mod.rs` lines 309-346 has `create_driver_with_orchestration()` — a function that routes tasks to the optimal model based on task type (research → deep model, coding → code model, quick Q&A → fast model). The orchestrator config IS created in kernel.rs, but this function is never called. The dumb `create_driver()` is used everywhere instead.

At 100 agents, this means every agent uses the same expensive model for everything — even "what time is it?" questions.

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Wire up the existing model orchestrator so agents automatically route to the optimal model per task.

1. Find `create_driver_with_orchestration()` in crates/openfang-runtime/src/drivers/mod.rs (around line 309-346). Read it carefully — understand what it does.

2. Find where `create_driver()` is called in the agent loop (crates/openfang-runtime/src/agent_loop.rs) and in the kernel (crates/openfang-kernel/src/kernel.rs). These are the call sites that need to optionally use the orchestrator.

3. Find the ModelOrchestrator creation in kernel.rs (around line 915-920). It's already created and stored. Verify it's accessible from the agent loop.

4. Modify the agent loop so that when orchestrator is enabled:
   - BEFORE calling the LLM, classify the user message using the orchestrator
   - Use `create_driver_with_orchestration()` to get the optimal driver for that task type
   - Fall back to the agent's default driver if orchestration fails or returns None
   - Log: tracing::info!(task_type = %classified, model = %selected_model, "Orchestrator routed to model")

5. Make sure orchestration is OPTIONAL and off by default:
   - Config field already exists: orchestrator.enabled (verify this)
   - When disabled, use create_driver() as before (zero behavior change)
   - When enabled, use create_driver_with_orchestration()

6. Add Sentry span data for orchestration:
   - span.set_data("gen_ai.orchestrator.enabled", true/false)
   - span.set_data("gen_ai.orchestrator.task_type", classified_type)
   - span.set_data("gen_ai.orchestrator.selected_model", model_name)

7. Add to Prometheus metrics:
   - openfang_orchestrator_routes_total{task_type="research"} (counter per task type)
   - openfang_orchestrator_fallbacks_total (counter — when orchestration failed and fell back)

8. Write tests:
   - Test that with orchestrator disabled, create_driver() is used (default behavior)
   - Test that with orchestrator enabled, task classification returns a valid task type
   - Test that fallback works when orchestrator returns an error
   - Test that the correct model is selected for "research" vs "quick_qa" task types

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

# Confirm create_driver_with_orchestration is now CALLED (not just defined)
grep -rn 'create_driver_with_orchestration' crates/openfang-runtime/src/agent_loop.rs crates/openfang-kernel/src/kernel.rs
# Expected: at least 1 call site (not just the definition in mod.rs)

# Confirm orchestrator routing is in agent loop
grep -rn 'orchestrator.*enabled\|task_type\|orchestrat' crates/openfang-runtime/src/agent_loop.rs
# Expected: conditional check + routing logic

# Confirm Sentry span data
grep -rn 'gen_ai.orchestrator' crates/openfang-runtime/src/agent_loop.rs
# Expected: 3 span.set_data calls

# Confirm Prometheus metrics
grep -rn 'orchestrator_routes\|orchestrator_fallbacks' crates/openfang-api/src/routes.rs
# Expected: 2 metric families

# Run orchestrator tests
cargo test orchestrat -- --nocapture

# Live test (with orchestrator enabled in config.toml):
# [orchestrator]
# enabled = true
curl -s -X POST "http://127.0.0.1:4200/api/agents/YOUR_AGENT_ID/message" \
  -H "Content-Type: application/json" \
  -d '{"message": "What is 2+2?"}'
# Check daemon logs for: "Orchestrator routed to model"

curl -s http://127.0.0.1:4200/api/metrics | grep orchestrator
# Expected: orchestrator_routes_total and orchestrator_fallbacks_total
```
