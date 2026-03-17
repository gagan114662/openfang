# Contract 12: SQLite Connection Pool
**Agent:** Codex
**Branch:** `codex/12-sqlite-pool`

## Problem
`crates/openfang-memory/src/structured.rs` line 12 has:
```rust
pub struct StructuredStore {
    conn: Arc<Mutex<Connection>>,  // SINGLE connection, ALL agents block on this
}
```
At 100 concurrent agents, every write serializes through one lock. This is the #1 throughput bottleneck.

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Replace the single Mutex-wrapped SQLite connection with a connection pool.

1. Add the `r2d2` and `r2d2_sqlite` crates to openfang-memory/Cargo.toml
   (Alternative: use `deadpool-sqlite` if you prefer async. Either is fine.)

2. In crates/openfang-memory/src/structured.rs, replace:
   conn: Arc<Mutex<Connection>>
   with:
   pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>

3. Configure the pool:
   - min_idle: Some(2)
   - max_size: 20
   - connection_timeout: Duration::from_secs(10)
   - Enable WAL mode on each connection: PRAGMA journal_mode=WAL;
   - Enable busy timeout: PRAGMA busy_timeout=5000;

4. Add config fields to the appropriate config struct:
   - sqlite_pool_size: u32, default 20
   - sqlite_busy_timeout_ms: u32, default 5000
   Add #[serde(default)] and Default impl entries.

5. Update ALL methods on StructuredStore to get a connection from the pool:
   let conn = self.pool.get()?;
   instead of:
   let conn = self.conn.lock()?;

6. Do the same for EVERY other store that uses Arc<Mutex<Connection>>:
   - Check openfang-memory/src/a2a_tasks.rs
   - Check openfang-memory/src/usage.rs (or similar)
   - Check any other store that wraps a Connection
   They should ALL share the same pool.

7. Create a single pool at initialization (in kernel.rs or memory module init) and pass it to all stores.

8. Add Prometheus metrics to /api/metrics:
   - openfang_sqlite_pool_size (gauge) — total pool size
   - openfang_sqlite_pool_active (gauge) — active connections
   - openfang_sqlite_pool_idle (gauge) — idle connections

9. Write tests:
   - Test that 10 concurrent writes all succeed (spawn 10 tokio tasks, each writing)
   - Test that pool exhaustion returns an error (set pool size to 1, hold connection, try to get another with timeout)
   - Test that WAL mode is enabled (query PRAGMA journal_mode and assert "wal")

When done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. grep -rn 'Arc<Mutex<Connection>>' crates/openfang-memory/ returns ZERO results

You are NOT done until all four checks pass.
```

## Verification (you run this)

```bash
# The three mandatory checks
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Confirm no more Mutex<Connection> in memory crate
grep -rn 'Arc<Mutex<Connection>>' crates/openfang-memory/src/
# Expected: nothing

# Confirm pool is used
grep -rn 'pool\.get()' crates/openfang-memory/src/
# Expected: multiple matches across store files

# Confirm WAL mode
grep -rn 'journal_mode.*WAL\|journal_mode.*wal' crates/openfang-memory/src/
# Expected: at least 1 match

# Confirm config field
grep -rn 'sqlite_pool_size' crates/
# Expected: config definition + memory init

# Live test: start daemon, check metrics
curl -s http://127.0.0.1:4200/api/metrics | grep sqlite_pool
# Expected: openfang_sqlite_pool_size 20

# Stress test: fire 50 concurrent agent messages
for i in $(seq 1 50); do
  curl -s -X POST "http://127.0.0.1:4200/api/agents/YOUR_AGENT_ID/message" \
    -H "Content-Type: application/json" \
    -d '{"message": "Say hi"}' &
done
wait
# Check no errors in daemon logs related to SQLite locking
```
