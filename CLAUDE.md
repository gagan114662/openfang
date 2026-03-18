# OpenFang — Agent Instructions

## Project Overview
OpenFang is an open-source Agent Operating System written in Rust (14 crates).
- Config: `~/.openfang/config.toml`
- Default API: `http://127.0.0.1:4200`
- CLI binary: `target/release/openfang.exe` (or `target/debug/openfang.exe`)

## Claude Desktop Workflow
- Repo-local MCP config lives in `.mcp.json` and exposes:
  - `openfang` via `cargo run --quiet -p openfang-cli -- mcp`
  - `contextplus` for local semantic context
- Repo-local Claude hooks live in `.claude/settings.json` and wrap `entire` through `scripts/claude/claude_hook.py`.
- Canonical Claude lifecycle events are emitted as:
  - `claude.session.started`
  - `claude.session.ended`
  - `claude.session.stopped`
  - `claude.task.started`
  - `claude.task.completed`
  - `claude.task.failed`
  - `claude.prompt.submitted`
- MCP tool calls from Claude Desktop are observable as:
  - `mcp.tool_call.started`
  - `mcp.tool_call.completed`
  - `mcp.tool_call.failed`

## Sentry Queries
- API heartbeat: `event.kind:api.request`
- Agent loops: `event.kind:runtime.agent_loop.completed OR event.kind:runtime.agent_loop.failed`
- LLM calls: `event.kind:runtime.llm_call.completed OR event.kind:runtime.llm_call.failed`
- Claude hooks: `event.kind:claude.*`
- Claude Desktop MCP: `event.kind:mcp.tool_call.*`
- Desktop app lifecycle: `event.kind:desktop.lifecycle.*`
- For live Sentry summaries, do not hand-roll ad hoc curl queries or use the broken `eventsv2` path.
- Use `python3 scripts/harness/sentry_live_summary.py --config ~/.openfang/config.toml --stats-period 24h --format json`.
- Trust `issues.groups_seen_24h`, `issues.unresolved_groups_seen_24h`, `errors.count_24h`, and `transactions.*` from that helper rather than raw issue counts.

## Build & Verify Workflow
After every feature implementation, the required finish gate is:
```bash
cargo build --workspace --lib          # Must compile (use --lib if exe is locked)
cargo test --workspace                 # All tests must pass (currently 1744+)
```
`cargo clippy --workspace --all-targets -- -D warnings` is still recommended, but it is not part of the enforced session-finish gate.

## MANDATORY: Live Integration Testing
**After implementing any new endpoint, feature, or wiring change, you MUST run live integration tests.** Unit tests alone are not enough — they can pass while the feature is actually dead code. Live tests catch:
- Missing route registrations in server.rs
- Config fields not being deserialized from TOML
- Type mismatches between kernel and API layers
- Endpoints that compile but return wrong/empty data

### How to Run Live Integration Tests

#### Step 1: Stop any running daemon
```bash
tasklist | grep -i openfang
taskkill //PID <pid> //F
# Wait 2-3 seconds for port to release
sleep 3
```

#### Step 2: Build fresh release binary
```bash
cargo build --release -p openfang-cli
```

#### Step 3: Start daemon with required API keys
```bash
GROQ_API_KEY=<key> target/release/openfang.exe start &
sleep 6  # Wait for full boot
curl -s http://127.0.0.1:4200/api/health  # Verify it's up
```
The daemon command is `start` (not `daemon`).

#### Step 4: Test every new endpoint
```bash
# GET endpoints — verify they return real data, not empty/null
curl -s http://127.0.0.1:4200/api/<new-endpoint>

# POST/PUT endpoints — send real payloads
curl -s -X POST http://127.0.0.1:4200/api/<endpoint> \
  -H "Content-Type: application/json" \
  -d '{"field": "value"}'

# Verify write endpoints persist — read back after writing
curl -s -X PUT http://127.0.0.1:4200/api/<endpoint> -d '...'
curl -s http://127.0.0.1:4200/api/<endpoint>  # Should reflect the update
```

#### Step 5: Test real LLM integration
```bash
# Get an agent ID
curl -s http://127.0.0.1:4200/api/agents | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['id'])"

# Send a real message (triggers actual LLM call to Groq/OpenAI)
curl -s -X POST "http://127.0.0.1:4200/api/agents/<id>/message" \
  -H "Content-Type: application/json" \
  -d '{"message": "Say hello in 5 words."}'
```

#### Step 6: Verify side effects
After an LLM call, verify that any metering/cost/usage tracking updated:
```bash
curl -s http://127.0.0.1:4200/api/budget       # Cost should have increased
curl -s http://127.0.0.1:4200/api/budget/agents  # Per-agent spend should show
```

#### Step 7: Verify dashboard HTML
```bash
# Check that new UI components exist in the served HTML
curl -s http://127.0.0.1:4200/ | grep -c "newComponentName"
# Should return > 0
```

#### Step 8: Cleanup
```bash
tasklist | grep -i openfang
taskkill //PID <pid> //F
```

### Key API Endpoints for Testing
| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/health` | GET | Basic health check |
| `/api/agents` | GET | List all agents |
| `/api/agents/{id}/message` | POST | Send message (triggers LLM) |
| `/api/budget` | GET/PUT | Global budget status/update |
| `/api/budget/agents` | GET | Per-agent cost ranking |
| `/api/budget/agents/{id}` | GET | Single agent budget detail |
| `/api/network/status` | GET | OFP network status |
| `/api/peers` | GET | Connected OFP peers |
| `/api/a2a/agents` | GET | External A2A agents |
| `/api/a2a/discover` | POST | Discover A2A agent at URL |
| `/api/a2a/send` | POST | Send task to external A2A agent |
| `/api/a2a/tasks/{id}/status` | GET | Check external A2A task status |

## Architecture Notes
- **Don't touch `openfang-cli`** — user is actively building the interactive CLI
- `KernelHandle` trait avoids circular deps between runtime and kernel
- `AppState` in `server.rs` bridges kernel to API routes
- New routes must be registered in `server.rs` router AND implemented in `routes.rs`
- Dashboard is Alpine.js SPA in `static/index_body.html` — new tabs need both HTML and JS data/methods
- Config fields need: struct field + `#[serde(default)]` + Default impl entry + Serialize/Deserialize derives

## MANDATORY: Clean At Rest
**Agent sessions must end clean.** OpenFang now enforces this with a strict finish gate:
1. No tracked or unignored untracked dirt may remain when the session ends.
2. Docs-only sessions may finish clean without Rust validation.
3. Code-bearing sessions must pass `cargo build --workspace --lib` and `cargo test --workspace`.
4. The normal path is refusal, not auto-commit.

## MANDATORY: Use Tool-Specific Worktrees
**Do not share a writable checkout with Codex.** For any active task, open a dedicated linked worktree first:
```bash
# Claude task: create/reopen worktree and launch Claude there
of-claude <task-name>

# Codex task: create/reopen worktree and launch Codex there
of-codex <task-name>

# See which worktrees are clean vs dirty
bash scripts/worktree/status.sh

# Inspect or change root checkout lock state for human maintenance
bash scripts/worktree/root_mode.sh status
bash scripts/worktree/root_mode.sh unlock
bash scripts/worktree/root_mode.sh lock
```
Rules:
- Root checkout is inspection/integration only and is read-only by default.
- Launch is blocked if the root checkout has disallowed uncommitted changes. Only local metadata/log paths such as `.claude/**`, `.codex/**`, `.entire/**`, `artifacts/**`, `log/**`, `*.log`, and `.DS_Store` are ignored by default.
- Claude edits belong on `claude/<task>` worktrees.
- Codex edits belong on `codex/<task>` worktrees.
- Never run Claude and Codex in the same worktree at the same time.
- Raw `claude` and `codex` inside any OpenFang checkout or linked worktree prompt for a task name, then auto-route into the matching managed worktree.
- Claude hook policy rejects sessions that were not launched through the guarded worktree path.
- Locks live outside the repo and prevent concurrent Claude/Codex sessions in the same worktree.
- Root lock auto-syncs the clean canonical branch to the configured remote before making the checkout read-only. Override the target with `OPENFANG_CANONICAL_PUSH_REMOTE` and `OPENFANG_CANONICAL_PUSH_BRANCH`. Extend the ignored-root list with `OPENFANG_ROOT_DIRTY_ALLOWLIST` using `:`-separated shell globs.

## Common Gotchas
- `openfang.exe` may be locked if daemon is running — use `--lib` flag or kill daemon first
- `PeerRegistry` is `Option<PeerRegistry>` on kernel but `Option<Arc<PeerRegistry>>` on `AppState` — wrap with `.as_ref().map(|r| Arc::new(r.clone()))`
- Config fields added to `KernelConfig` struct MUST also be added to the `Default` impl or build fails
- `AgentLoopResult` field is `.response` not `.response_text`
- CLI command to start daemon is `start` not `daemon`
- On Windows: use `taskkill //PID <pid> //F` (double slashes in MSYS2/Git Bash)
