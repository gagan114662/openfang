# Autofix Safety Policy

Date: 2026-03-05
Status: active

## Purpose

This document defines what the unattended `ops-coder` loop may change without human review.
If a proposed change is outside this policy, the loop must stop, record a blocker, and escalate over Telegram instead of guessing.

## Allowed Unattended Fix Classes

- structured logging schema/queryability fixes
- guard/remediation logic fixes
- duplicate Telegram poller ownership fixes
- retry/backoff fixes
- auth command wiring and persistence fixes
- deterministic deploy/config/script fixes
- targeted runtime/API glue fixes for the unattended loop
- small bounded build-break repairs caused by type/config drift
- targeted test fixture updates required by the above

## Disallowed Unattended Fix Classes

- broad refactors
- new speculative product features
- destructive schema migrations
- secret/account/business-logic changes
- large API redesigns
- anything that depends on taste-based product judgment
- multi-file behavioral changes without a deterministic failing signal

## Required Validation Gate

Every unattended code change must pass all applicable gates before deploy:

1. targeted `cargo check`
2. targeted tests for touched surfaces
3. script syntax validation where relevant
4. post-deploy health verification
5. fresh Sentry verification query for the changed path

If any gate fails:

- do not keep the new deploy live
- emit `ops.autofix.failed` and `ops.deploy.failed`
- attempt rollback if a rollout already happened
- write the blocker into `artifacts/autonomy/current-state.json`

## Deployment Rules

- deploy target is the GPU host primary runtime
- deploys must be logged to `artifacts/autonomy/deploy-history.jsonl`
- every deploy must emit `ops.deploy.started` and either `ops.deploy.completed` or `ops.deploy.failed`
- if health or Sentry verification regresses, rollback immediately

## Browser Session Rules

- primary Sentry browser host: GPU host
- fallback Sentry browser host: Mac
- losing both browser sessions means triage is blind; record a blocker and escalate
- fallback browser usage must emit `ops.sentry.browser_fallback`

## Telegram Escalation Boundary

Escalate instead of auto-fixing when the issue requires:

- manual login
- 2FA
- new credentials
- secret rotation
- ambiguous product behavior
- unsafe or large changes
