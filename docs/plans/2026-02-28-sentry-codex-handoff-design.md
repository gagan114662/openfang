# Sentry → Codex Auto-Remediation Handoff

## Summary

Three modes for handing Sentry issues to Codex for automated fixing:
1. **Immediate MCP** — Use Codex MCP tool in Claude Code sessions
2. **CLI script** — `scripts/sentry_to_codex.sh` for on-demand use
3. **Webhook** — Sentry POSTs to OpenFang API + GitHub Actions dispatch

## Mode 1: Immediate MCP

No code changes. Call `mcp__codex__codex` with Sentry issue context.

## Mode 2: CLI Script

`scripts/sentry_to_codex.sh`:
- Fetch findings via `sentry_findings.py`
- For each actionable finding, call `codex exec --json --approval-mode full-auto`
- Validate with `cargo build && cargo test && cargo clippy`
- Commit and open PR via `gh`
- One PR per issue for clean reverts

## Mode 3a: GitHub Actions

Fill `OPENFANG_SENTRY_REMEDIATION_CMD` in existing workflow with Codex command.

## Mode 3b: OpenFang Webhook

- `POST /api/webhooks/sentry` — receives Sentry webhook events
- Verifies HMAC-SHA256 signature
- Normalizes to findings JSON schema
- Dispatches `repository_dispatch` to GitHub
- Stores event for dashboard visibility

## Guardrails

From `policy.contract.json`:
- Allowed paths: `crates/**/src/**/*.rs`, `tests/**/*.rs`, `docs/**/*.md`
- Blocked paths: `.git/**`, `.github/**`, `target/**`, `Cargo.lock`
- Max 10 files per PR, 500 lines per file
- Max 1 remediation attempt per SHA

## Autonomy

Full auto: Codex fixes, validates (build+test+clippy), opens PR. Human reviews PR.
