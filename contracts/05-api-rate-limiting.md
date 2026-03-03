# Contract 05: API Rate Limiting
**Agent:** Claude
**Branch:** `claude/05-rate-limiting`

## Prompt (copy-paste this into Claude)

```
Read CLAUDE.md first.

Your task: Add rate limiting middleware to the HTTP API.

The API server in crates/openfang-api/src/server.rs has no rate limiting. The governor crate is already a workspace dependency.

1. Create a tower-compatible rate limiting middleware using governor
2. Default: 100 requests per minute per IP address
3. Add a config field api_rate_limit_per_minute to the appropriate config struct with #[serde(default)] defaulting to 100
4. Add the default to the Default impl
5. Apply the middleware to all /api/ routes in server.rs
6. When rate limited, return HTTP 429 Too Many Requests with a JSON body: {"error": "rate_limit_exceeded", "retry_after_seconds": N}
7. Do NOT rate limit the health check endpoint /api/health
8. Write a test that sends requests in a loop and verifies 429 is returned after the limit

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

# Live test (after starting daemon):
# Hammer the endpoint and check for 429s
for i in $(seq 1 110); do
  curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:4200/api/agents
done | sort | uniq -c
# Should show some 429 responses
```
