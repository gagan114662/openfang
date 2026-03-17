# Rust Test Harness Service

HTTP harness for compiling/testing Rust code in Docker with Sentry reporting.

## What it does

- Accepts agent-submitted Rust project code (`source_path` or `source_tar_gz_base64`).
- Runs compile/test checks inside an isolated Docker container (`--network none`, dropped caps, pids/memory/cpu limits).
- Returns per-check pass/fail results over HTTP.
- Emits run outcome telemetry to Sentry.

## Start the service

```bash
python3 scripts/harness/rust_test_harness_service.py --host 127.0.0.1 --port 7788
```

Environment variables:

- `RUST_HARNESS_DOCKER_IMAGE` (default `rust:1.86-bookworm`)
- `RUST_HARNESS_DEFAULT_TIMEOUT_SECS` (default `900`)
- `RUST_HARNESS_ALLOWED_ROOTS` (comma-separated allowed host roots for `source_path`)
- `RUST_HARNESS_RESULTS_DIR` (default `artifacts/rust-harness-runs`)
- `RUST_HARNESS_SENTRY_DSN_ENV` (default `SENTRY_DSN`)
- `SENTRY_DSN` (or custom DSN env name above)

## API

### `GET /health`

Returns service + Docker readiness.

### `POST /v1/rust-harness/run`

Request body:

```json
{
  "source_path": "/absolute/path/to/rust/project",
  "checks": [
    {"name": "build", "command": "cargo build --workspace --all-targets"},
    {"name": "test", "command": "cargo test --workspace --all-targets"}
  ],
  "timeout_secs": 900,
  "continue_on_failure": false,
  "env": {
    "RUSTFLAGS": "-D warnings"
  }
}
```

Inline upload mode:

```json
{
  "source_tar_gz_base64": "<base64 tar.gz>",
  "checks": [
    {"name": "build", "command": "cargo build"},
    {"name": "test", "command": "cargo test"}
  ]
}
```

Response includes:

- run status (`passed`/`failed`)
- per-check outcomes and exit codes
- stdout/stderr tails
- isolation config used
- Sentry delivery status
- artifact path for stored run report JSON

