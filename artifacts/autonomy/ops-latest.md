# Ops Latest

Generated: 2026-03-06 13:12 EST

## What Changed
- Live GPU timer updated to 15-second cadence.
- Remote `openfang-vacation-guard.timer` now has `AccuracySec=1s`, which removed the previous 1-minute systemd coalescing.
- `scripts/remote_deploy_control_plane.sh` now writes the 15-second timer and the matching autonomy defaults.

## Verified Now
- Remote `openfang.service` is active.
- Remote `openfang_control_plane.service` is inactive (duplicate Telegram poller prevented).
- Remote vacation guard timer is active at 15 seconds.
- Consecutive guard heartbeats on the live host were observed at:
  - `2026-03-06T18:11:50.129406736+00:00`
  - `2026-03-06T18:12:06.135348115+00:00`
- Current known-good heartbeat request id:
  - `aa3a5400-f212-48a9-a52d-dcddd51d0980`
- Current known-good synthetic trace transaction:
  - `ops.guard.cycle`

## Blocked
- Full Rust patch-set validation is still pending local `cargo check`.
- The code-side deploy for panic hook, HTTP Sentry layer, Telegram durable outbox, and auto-restart has not been rolled to the GPU host yet in this pass.

## Next Recovery Step
- Finish local build validation, deploy the updated daemon binary, then re-verify Sentry `Logs` and `Traces` against the live 15-second guard cadence.
