# Contract 09: A2A Dashboard Tab
**Agent:** Claude
**Branch:** `claude/09-a2a-dashboard`

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Add a "Network" tab to the dashboard for A2A agent management.

The dashboard is an Alpine.js SPA in crates/openfang-api/static/index_body.html. Look at how existing tabs (like "Channels" or "Skills") are implemented and follow the exact same pattern.

1. Add a new tab called "Network" (or "A2A") to the tab navigation
2. The tab should have three sections:

   Section 1 — Discovered Agents:
   - Fetch from GET /api/a2a/agents on tab load
   - Show a table/list with columns: agent name, URL, capabilities, status
   - Show "No agents discovered yet" if empty

   Section 2 — Discover New Agent:
   - An input field for agent URL
   - A "Discover" button that POSTs to /api/a2a/discover with { "url": "<input>" }
   - Show success/error feedback
   - Refresh the agents list after successful discovery

   Section 3 — Task History:
   - Fetch from GET /api/a2a/tasks (you may need to add this endpoint if it doesn't exist)
   - Show a table with: task ID, agent, status (with color badges), created time
   - If the endpoint doesn't exist, create it in routes.rs and register it in server.rs

3. Match the existing dashboard styling exactly — same card classes, same color scheme, same Alpine.js patterns
4. Use x-data, x-init, x-show, x-for etc. consistently with other tabs

When you're done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes
4. curl -s http://127.0.0.1:4200/ | grep -c 'Network\|a2a' returns > 0

You are NOT done until all four checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Check the HTML contains the new tab
curl -s http://127.0.0.1:4200/ | grep -c 'Network\|a2a'
# Should be > 0

# Visual check: open http://127.0.0.1:4200 in browser, click Network tab
```
