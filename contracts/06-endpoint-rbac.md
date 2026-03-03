# Contract 06: Per-Endpoint RBAC
**Agent:** Codex
**Branch:** `codex/06-endpoint-rbac`

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Add role-based access control to the API endpoints.

Currently the API in crates/openfang-api/ only validates that a Bearer token exists. All authenticated requests have the same access level.

1. Define 3 roles in the config: admin, operator, viewer
2. In the config struct, each API token should map to a role. Example config.toml structure:
   [api]
   tokens = [
     { token = "abc123", role = "admin" },
     { token = "def456", role = "viewer" }
   ]
3. Role permissions:
   - viewer: GET only (read endpoints)
   - operator: GET + POST (read + send messages, trigger actions)
   - admin: GET + POST + PUT + DELETE (full access including config, budget changes)
4. Create a middleware or extractor in crates/openfang-api/src/middleware.rs that:
   - Extracts the Bearer token from the request
   - Looks up the token's role from config
   - Checks the HTTP method against the role's allowed methods
   - Returns 403 Forbidden with JSON body if insufficient role
5. Apply this middleware to all /api/ routes (except /api/health which stays public)
6. Write tests for:
   - viewer token doing GET → 200
   - viewer token doing POST → 403
   - operator token doing POST → 200
   - operator token doing DELETE → 403
   - admin token doing DELETE → 200
7. Maintain backward compatibility: if no tokens config exists, fall back to current behavior (any Bearer token = admin)

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
cargo test rbac -- --nocapture
# Should show role-based tests passing
```
