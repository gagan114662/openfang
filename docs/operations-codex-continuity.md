# Codex Continuity Runbook

Date: 2026-03-06
Status: active

## Latest Live State (2026-03-06 13:12 EST)

What is verified now:
- GPU host `192.168.40.234` is the sole Telegram poller.
- `openfang-vacation-guard.timer` is configured with:
  - `OnUnitActiveSec=15`
  - `AccuracySec=1s`
- Guard heartbeats are now advancing every ~15-16 seconds on the live host.
- Verified consecutive guard heartbeat/request IDs:
  - `994b024d-6cab-4be7-a8ef-fa07438f0919` at `2026-03-06T18:11:50.129406736+00:00`
  - `aa3a5400-f212-48a9-a52d-dcddd51d0980` at `2026-03-06T18:12:06.135348115+00:00`
- Canonical trace transaction for those heartbeats: `ops.guard.cycle`

Use these Sentry checks first:
- `event.kind:ops.guard.heartbeat`
- `transaction:ops.guard.cycle`
- `request.id:aa3a5400-f212-48a9-a52d-dcddd51d0980`

## Previous Automation Run (2026-03-05 18:11 EST)

Scope executed from repo:
- `scripts/codex_live_state.sh`
- `scripts/harness/vacation_guard.py --api-base http://127.0.0.1:50051 --enforce-single-poller`
- direct endpoint probes for:
  - `GET /api/auth/preflight`
  - `POST /api/ops/guard/report`
  - `POST /ops/guard/report`

What changed in this run:
- Fixed `scripts/codex_live_state.sh` to correctly tail the remote guard artifact path (`$HOME/open_fang/...`) over SSH.
- Hardened `scripts/harness/vacation_guard.py` guard reporting:
  - tries both `/api/ops/guard/report` and `/ops/guard/report`
  - records attempted paths + used path in `report.sentry.guard_report_paths_tried`
  - includes explicit Sentry query-shape note for runtime message content (`payload.input.user_message`)

What was verified:
- Local daemon healthy on `http://127.0.0.1:50051/api/health` (`200`).
- Remote `openfang.service` is `active`.
- Remote `openfang_control_plane.service` is `inactive` (single-poller ownership preserved).
- Remote guard timer is enabled/running.
- Fresh local guard artifact now fails closed when guard report delivery fails:
  - `artifacts/vacation-guard/latest.json` includes `status: "fail"` and both attempted report paths with `404`.

What is blocked:
- Active daemons still return `404` for guard/auth ops endpoints:
  - `GET /api/auth/preflight`
  - `POST /api/ops/guard/report`
  - `POST /ops/guard/report`
- Sentry API validation is blocked locally by missing `SENTRY_AUTH_TOKEN`:
  - see `artifacts/sentry-logs-validation-live.json`.
- Remote guard service output still shows older behavior (`status: "pass"` with `guard_report_status_code: 404`), indicating deployed script/binary drift from current repo.

## Purpose

This file is the durable source of truth for the current OpenFang live-operating setup.
It exists so work can resume from the repo even if chat context is gone.

## Current Topology

Target unattended topology:
- Primary daemon host: GPU host `gagan-arora@192.168.40.234`
- Primary Telegram owner: `openfang.service` on the GPU host
- Guard owner: `openfang-vacation-guard.timer` on the GPU host
- Local Mac role: secondary console and development box only

Current deploy path in repo:
- remote deploy script: `scripts/remote_deploy_control_plane.sh`
- guard script: `scripts/harness/vacation_guard.py`

Telegram ownership rule:
- Exactly one poller may be active.
- `openfang.service` is the intended long-poller for unattended mode.
- `openfang_control_plane.service` must stay disabled/stopped unless ownership is deliberately moved.

## Sentry Logging State

Structured Sentry logs are wired through direct `capture_log(...)` calls and a filtered tracing layer.

Key implementation points:
- Workspace Sentry SDK upgraded from `0.34` to `0.40`.
- `enable_logs = true` is set in kernel Sentry initialization.
- API request middleware emits canonical `api.request` logs.
- Runtime emits canonical logs for:
  - `runtime.llm_call.completed`
  - `runtime.llm_call.failed`
  - `runtime.agent_loop.completed`
  - `runtime.agent_loop.failed`
- CLI tracing forwards `WARN` and `ERROR` to Sentry while canonical `INFO` logs use explicit `capture_log(...)`.

Expected Sentry filters:
- `event.kind:api.request`
- `event.kind:runtime.llm_call.completed`
- `event.kind:runtime.llm_call.failed`
- `event.kind:runtime.agent_loop.completed`
- `event.kind:runtime.agent_loop.failed`
- `event.kind:ops.guard.heartbeat`
- `event.kind:ops.guard.check_failed`
- `event.kind:ops.guard.remediated`
- `event.kind:ops.guard.failed`
- `event.kind:auth.preflight.completed`
- `event.kind:auth.preflight.failed`
- `event.kind:auth.escalation.sent`
- `event.kind:auth.escalation.resolved`

Verified live on 2026-03-05 in the Sentry dashboard:
- Structured rows are visible in project `openfang-monitoring`.
- Runtime user-message content is searchable under `payload.input.user_message`, not `input.user_message`.
- Example proven query:
  - `event.kind:runtime.agent_loop.failed payload.input.user_message:*SENTRY_DIRECT_PROBE_20260305_1*`
- Example proven attributes on a live row:
  - `agent.id`
  - `agent.name`
  - `session.id`
  - `provider`
  - `model`
  - `failure_reason`
  - `outcome`
  - `payload.input.user_message`
  - `payload.output.response`

## Known Live Constraints

These warnings are currently expected on boot:
- `EMAIL_PASSWORD` not set
- `OPENFANG_RLM_PG_MAIN_DSN` not set
- local providers like `vllm` or `lmstudio` may show offline warnings

These do not block Telegram ownership or Sentry structured logs.

## Current State

What is now wired:
- `GET /api/auth/preflight` runs provider/browser/SSH/token readiness checks.
- `GET /api/autonomy/state` returns the durable unattended topology/state snapshot.
- `POST /api/autonomy/deploy/report` appends the unattended deploy ledger and emits canonical deploy logs.
- `POST /api/autonomy/triage/report` persists the latest triage summary and emits canonical triage logs.
- `POST /api/ops/guard/report` emits canonical `ops.guard.*` Sentry logs.
- Telegram/channel commands now support:
  - `/auth-preflight`
  - `/auth [agent] <service> <credential>`
  - `/resume-auth [agent] <service>`
  - `/auth-status`
  - `/ops-status`
  - `/latest-fix`
  - `/latest-brief`
- Structured KV now supports TTL-backed entries via `expires_at`.
- Browser config now supports:
  - `user_data_dir`
  - `browser_executable`
  - `extensions_dir`
  - `cookie_backup_interval_secs`
- Sentry config now supports:
  - `wide_event_attribute_max_bytes`
  - `wide_event_payload_max_bytes`
- The guard emits `ops.guard.*` events and can:
  - restart remote `openfang.service`
  - stop remote `openfang_control_plane.service`
- The unattended workload registry now lives at:
  - `config/unattended_workloads.toml`
- The unattended autofix safety policy now lives at:
  - `docs/ops/autofix-safety-policy.md`
- The native schedule bootstrap script now lives at:
  - `scripts/seed_unattended_workloads.py`

Still not fully closed:
- Automatic Telegram push for auth failures is not yet end-to-end; auth escalation state is persisted and queryable, and manual `/auth` recovery is wired.
- Provider quarantine/reroute is not yet fully integrated into the agent runtime loop.

## Reconstruction Procedure

If chat context is lost, do this first:

1. Run the live-state script:
   - `./scripts/codex_live_state.sh`
2. Confirm local daemon:
   - `curl http://127.0.0.1:50051/api/health`
3. Confirm local Telegram connection:
   - `lsof -Pan -p $(pgrep -f "target/debug/openfang start") -i`
4. Confirm remote control plane is not polling Telegram:
   - `ssh gagan-arora@192.168.40.234 "systemctl --user status openfang_control_plane.service --no-pager"`
5. Confirm remote guard timer:
   - `ssh gagan-arora@192.168.40.234 "systemctl --user status openfang-vacation-guard.timer --no-pager"`
6. Check Sentry for fresh events using the filters above.
7. Check unattended autonomy state:
   - `curl http://127.0.0.1:50051/api/autonomy/state`
8. Check seeded workloads:
   - `sed -n '1,200p' config/unattended_workloads.toml`
   - `python3 scripts/seed_unattended_workloads.py --api-base http://127.0.0.1:50051`
9. When validating Telegram-originated activity, do not expect `api.request` rows.
   Telegram bypasses HTTP and should be checked with:
   - `event.kind:runtime.agent_loop.completed`
   - `event.kind:runtime.agent_loop.failed`
   - `event.kind:runtime.llm_call.completed`
   - `event.kind:runtime.llm_call.failed`

## Vacation Guard

There is now an unattended guard runner:
- script: `scripts/harness/vacation_guard.py`
- installer: `scripts/install_vacation_guard_launchd.sh`
- remote timer install path: `scripts/remote_deploy_control_plane.sh`

What each run does:
- hits `GET /api/status` locally to create a visible `api.request` heartbeat in Sentry
- records the returned `x-request-id`
- posts `ops.guard.heartbeat` or `ops.guard.check_failed` to `/api/ops/guard/report`
- checks local daemon liveness
- checks remote `openfang.service`
- checks remote `openfang_control_plane.service`
- when enabled with `--enforce-single-poller`, stops the remote control plane if it becomes active again
- restarts remote `openfang.service` when it is inactive
- posts remediation results as `ops.guard.remediated` or `ops.guard.failed`
- writes latest state to:
  - `artifacts/vacation-guard/latest.json`
- writes history snapshots to:
  - `artifacts/vacation-guard/history/*.json`

Expected cadence:
- every 5 minutes via launchd on the Mac today
- every 5 minutes via `openfang-vacation-guard.timer` on the GPU host after remote cutover

Sentry heartbeat query:
- take the latest `heartbeat_request_id` from `artifacts/vacation-guard/latest.json`
- search:
  - `event.kind:api.request request.id:<that-id>`

Sentry ops query:
- `event.kind:ops.guard.heartbeat`
- `event.kind:ops.guard.remediated remediation.action:stop_remote_control_plane`

## Operational Rules

- Do not run both the local Telegram poller and the remote control plane Telegram poller at the same time.
- Treat this file and `scripts/codex_live_state.sh` as the first recovery surface before relying on memory.
- When a new live-state decision is made, update this file in the same change set.

## Latest Live Check (2026-03-05 18:10 EST)

Reconstruction steps run in this order:
- Read this continuity doc.
- Ran `./scripts/codex_live_state.sh`.
- Inspected `artifacts/vacation-guard/latest.json`.
- Checked API status directly (`/api/health`, `/api/status`).
- Checked remote services and guard timer via `systemctl --user`.

Current observed state:
- Local daemon: healthy (`/api/health` 200, `/api/status` 200), PID `51600`.
- Remote daemon: healthy (`openfang.service` active), PID `190352`.
- Remote control plane: inactive (`openfang_control_plane.service` inactive).
- Remote guard timer: active (`openfang-vacation-guard.timer` waiting).
- Poller duplication: not detected (`openfang_control_plane.service` inactive, remote Telegram socket count `0`).
- Guard report endpoint: still returning `404` on both local and remote for `POST /api/ops/guard/report`.

Operational fix applied in repo:
- Updated `scripts/harness/vacation_guard.py` so a non-2xx/202 guard-report response is treated as a failure and included in `errors`, instead of reporting a false `pass`.
- Result: `artifacts/vacation-guard/latest.json` now correctly fails when guard reporting is unavailable.

Sentry validation status:
- Query-shape references remain:
  - `event.kind:api.request request.id:<heartbeat_request_id>`
  - `event.kind:runtime.agent_loop.completed`
  - `event.kind:runtime.agent_loop.failed`
  - `event.kind:runtime.llm_call.completed`
  - `event.kind:runtime.llm_call.failed`
- Live Sentry API verification is currently blocked because `SENTRY_AUTH_TOKEN` in this environment lacks `event:read` and `project:read` scopes.

## Immediate Next Priorities

1. Rebuild/redeploy the active daemon binary so `POST /api/ops/guard/report` is available on the running process.
2. Re-run `scripts/harness/vacation_guard.py --enforce-single-poller` and confirm `status: pass` with successful guard-report POST.
3. Verify fresh `ops.guard.*` and `auth.preflight.*` rows in Sentry once token scopes permit API-based validation.
4. Continue auth escalation delivery and provider quarantine/reroute integration work.
