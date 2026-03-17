# Structured Data Spine

OpenFang now persists canonical operational events locally in SQLite and mirrors the same wide events to Sentry.

Primary tables:

- `fact_events`: canonical event envelope plus indexed IDs for `event_kind`, `run_id`, `request_id`, `session_id`, `agent_id`, `trace_id`, and `outcome`
- `artifact_index`: artifact metadata linked back to `run_id`, `session_id`, and `agent_id`

Primary API endpoints:

- `GET /api/data/events`
- `GET /api/data/events/export`
- `GET /api/data/runs/{run_id}`

Saved Sentry query examples:

- `event.kind:api.request`
- `run.id:<id>`
- `request.id:<id>`
- `trace.id:<id>`
- `event.kind:ops.guard.heartbeat run.id:<id>`
- `event.kind:api.request default_provider.family:codex`
- `event.kind:runtime.agent_loop.completed`
- `event.kind:runtime.agent_loop.failed`
- `event.kind:runtime.llm_call.completed`
- `event.kind:runtime.llm_call.failed`
- `event.kind:auth.preflight.completed`
- `event.kind:auth.preflight.failed`
- `event.kind:auth.escalation.sent`
- `event.kind:auth.escalation.resolved`
- `event.kind:ops.guard.heartbeat`
- `event.kind:ops.guard.check_failed`
- `event.kind:ops.guard.remediated`
- `event.kind:ops.guard.failed`
- `event.kind:artifact.recorded`

Notes:

- SQLite is the authoritative store.
- Sentry is the first external query surface, not the only source of truth.
- Correlation IDs are indexed as Sentry tags for canonical events, so `Issues` queries can look up exact `run.id`, `request.id`, and `trace.id`.
- Existing event kinds are preserved where they already existed so current Sentry searches remain valid.
