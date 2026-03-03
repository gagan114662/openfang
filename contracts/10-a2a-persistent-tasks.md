# Contract 10: A2A Persistent Task Store
**Agent:** Codex
**Branch:** `codex/10-a2a-persistence`

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Move A2A task storage from in-memory DashMap to SQLite for persistence.

Currently A2A tasks are stored in a DashMap (in-memory). When the daemon restarts, all task history is lost.

1. Find the A2A task store — likely in crates/openfang-runtime/src/a2a.rs or similar
2. In the openfang-memory crate (crates/openfang-memory/), add a new module or extend existing storage:
   - Create table a2a_tasks with columns:
     - id TEXT PRIMARY KEY
     - agent_url TEXT NOT NULL
     - status TEXT NOT NULL (maps to task state enum)
     - request_json TEXT
     - response_json TEXT
     - error_message TEXT
     - created_at TEXT NOT NULL (ISO 8601)
     - updated_at TEXT NOT NULL (ISO 8601)
   - Add migration if the project uses a migration system, otherwise create table on init
3. Implement CRUD functions:
   - insert_a2a_task(task) -> Result<()>
   - get_a2a_task(id) -> Result<Option<A2aTask>>
   - list_a2a_tasks(limit, offset) -> Result<Vec<A2aTask>>
   - update_a2a_task_status(id, status, response_json) -> Result<()>
4. Wire the A2A runtime code to use SQLite instead of DashMap
5. Keep backward compatible — if SQLite is unavailable, log a warning and fall back to DashMap
6. Write tests:
   - Insert a task, retrieve it, verify fields match
   - Update task status, retrieve it, verify status changed
   - List tasks with limit/offset
   - Test that tasks survive a "restart" (drop and recreate the store, verify data persists)

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
cargo test a2a_persist -- --nocapture
# Should show persistence tests passing
```
