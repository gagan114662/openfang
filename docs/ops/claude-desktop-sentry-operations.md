# Claude Desktop + Sentry Operations

This repo is wired so Claude Desktop, OpenFang unattended workloads, and the desktop app all report into the same Sentry surface.

## Start Paths

Desktop app:
- `cargo run -p openfang-desktop`

Daemon:
- `cargo run -p openfang-cli -- start`

Claude Desktop MCP:
- repo-local `.mcp.json` exposes `openfang` with `cargo run --quiet -p openfang-cli -- mcp`

Claude hooks:
- `.claude/settings.json` routes Claude hook events through `scripts/claude/claude_hook.py`
- hook events are mirrored to `artifacts/claude/hook-events.jsonl`

## Canonical Queries

Liveness:
- `event.kind:api.request`
- `event.kind:desktop.lifecycle.server_ready`

Claude workspace:
- `event.kind:claude.*`
- `event.kind:mcp.tool_call.*`

Runtime:
- `event.kind:runtime.llm_call.completed OR event.kind:runtime.llm_call.failed`
- `event.kind:runtime.agent_loop.completed OR event.kind:runtime.agent_loop.failed`

Unattended ops:
- `event.kind:ops.guard.heartbeat`
- `event.kind:ops.guard.failed`
- `event.kind:ops.triage.completed`
- `event.kind:ops.deploy.completed`
- `event.kind:auth.preflight.completed`

## Safe vs Blocked

Safe unattended remediation:
- latest `ops.guard.*` events are fresh
- `runtime.agent_loop.failed` is not trending upward
- latest `ops.triage.*` result is success
- auth surface is green under `auth.preflight.completed`

Blocked unattended remediation:
- repeated `ops.guard.failed`
- missing recent `api.request` heartbeat
- repeated `runtime.llm_call.failed` from auth/billing/rate-limit causes
- fresh `auth.preflight.failed` or `auth.escalation.sent`
