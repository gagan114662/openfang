# Contract 03: Fix the Tool Calls Bug
**Agent:** Claude
**Branch:** `claude/03-fix-tool-calls`

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Fix the tool_calls extraction bug in the agent loop.

There is a comment near line 3108 in crates/openfang-runtime/src/agent_loop.rs that says "// BUG: no tool_calls!".

1. Read the surrounding code (at least 200 lines of context around that comment)
2. Understand the code path — when does tool_calls end up empty?
3. Identify the root cause. Common issues:
   - Response format differs between LLM providers
   - JSON path to tool_calls varies (some nest it, some don't)
   - Streaming responses might not aggregate tool calls correctly
4. Fix the root cause
5. Write at least 3 unit tests:
   - Test tool call extraction from a standard OpenAI-format response
   - Test tool call extraction from a response with zero tool calls (should return empty, not error)
   - Test tool call extraction from a streaming response that was aggregated
6. Put tests in the same file's test module

When you're done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes (including your new tests)
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. cargo test tool_call -- --nocapture shows your new tests running and passing

You are NOT done until all four checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test tool_call -- --nocapture
# Should see 3+ new tests passing
```
