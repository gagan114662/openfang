# OpenFang Harness Engineering

This document defines the deterministic PR agent loop for OpenFang.

## Control Plane

The harness is governed by one machine-readable contract:

- `.harness/policy.contract.json`

This contract controls:

- risk tiers by changed path
- required checks per tier
- docs drift rules
- review-policy semantics
- rerun-comment dedupe marker
- constrained remediation guardrails
- browser evidence requirements
- phased rollout policy

## Deterministic Order

PR sequence is strictly ordered:

1. `risk-policy-gate` runs first on PR open/sync.
2. Gate computes risk tier and required checks from changed files.
3. Gate validates current-head `greptile-review` state.
4. Gate emits `risk-policy-report.json` + normalized `review-findings.json`.
5. If gate passes, `ci-fanout` runs only required CI jobs.
6. If gate returns `needs-remediation`, `remediation-agent` can patch in-branch (constrained).
7. If gate returns stale or timeout review states, `greptile-rerun` posts one deduped rerun comment per SHA.
8. After clean rerun, `greptile-auto-resolve-threads` can resolve bot-only unresolved threads.
9. `sentry-remediation-agent` (scheduled or manual) can ingest unresolved Sentry issues, normalize findings, and open constrained remediation PRs.

## Current-Head SHA Discipline

Review is valid only when tied to current PR head SHA:

- stale review states are rejected
- missing current-head review fails policy semantics
- timeout and non-success review states are treated as gate failures
- rerun requests are deduplicated using marker + `sha:<head_sha>`

## Required Artifacts

### `risk-policy-report.json`

Produced by `scripts/harness/risk_policy_gate.py`.

Fields:

- `pr_number`, `head_sha`, `risk_tier`, `changed_files`
- `required_checks`
- `review_state`
- `actionable_findings_count`
- `decision` (`pass|fail|needs-remediation|stale-review|timeout`)
- `reasons`, `timestamp`

### `review-findings.json`

Normalized findings payload consumed by remediation.

### `browser-evidence-manifest.json`

Validated for UI-impacting changes by `scripts/harness/browser_evidence_verify.py`.

### `remediation-result.json`

Produced by `scripts/harness/remediation_runner.py`.

## Workflows

- `.github/workflows/risk-policy-gate.yml`
- `.github/workflows/ci-fanout.yml`
- `.github/workflows/greptile-rerun.yml`
- `.github/workflows/remediation-agent.yml`
- `.github/workflows/greptile-auto-resolve-threads.yml`
- `.github/workflows/harness-weekly-metrics.yml`
- `.github/workflows/sentry-remediation-agent.yml`

## Rollout

Configured in contract `rolloutPolicy`:

- `phase-0`: advisory baseline and metrics
- `phase-1`: block stale review/docs drift, run remediation only for PRs labeled `harness-remediation-pilot`
- `phase-2`: enforce constrained remediation + evidence
- `phase-3`: hard enforcement

Change `rolloutPolicy.currentPhase` to progress enforcement.

### Transitional Fanout Rule

In `phase-1`, CI fanout is still allowed after gate failure (`runFanoutOnGateFailure: true`) for observability while merge remains blocked by `risk-policy-gate`.
In `phase-2+`, fanout runs only after a passing gate.

## Local Repro Commands

```bash
python3 scripts/harness/risk_policy_gate.py --pr <n> --head-sha <sha> --changed-files <file>
python3 scripts/harness/browser_evidence_verify.py --manifest artifacts/browser-evidence-manifest.json
python3 scripts/harness/remediation_runner.py --findings artifacts/review-findings.json --head-sha <sha>
python3 scripts/harness/sentry_findings.py --org <org> --project <project> --query "is:unresolved level:error"
```

## Notes

- Gate and fanout are designed to avoid spending CI time on PR heads already blocked by policy.
- Remediation is constrained to contract-allowed paths and forbids control-plane bypass changes.
- Weekly metrics track stale-review rate, rerun pressure, remediation performance, and high-tier pass rate.
