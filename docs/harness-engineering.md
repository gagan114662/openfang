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
- review provider orchestration (`reviewProviders`)
- Claude + Codex remediation automation (`automation.claudeRemediation`)

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
10. `pr-review-harness` runs on every PR and publishes acceptance checklist + screenshot/video evidence to the PR body/comment.
11. `claude-remediation-agent` runs on every PR sync, ingests trusted Claude findings for the current head SHA, and applies constrained Codex remediation when actionable findings exist.

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

### `claude-findings.json`

Normalized findings payload ingested from trusted PR comments/reviews with provider `claude`.
The contract uses explicit allowlists (`trustedActorLogins`/`trustedAppIds`) and defaults to empty lists for safe deny-by-default behavior.

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
- `.github/workflows/pr-review-harness.yml`
- `.github/workflows/claude-remediation-agent.yml`

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
python3 scripts/harness/claude_feedback_ingest.py --repo <owner/repo> --pr <n> --head-sha <sha> --token-env GITHUB_TOKEN
python3 scripts/harness/evidence_capture.py --head-sha <sha> --changed-files <file>
python3 scripts/harness/pr_packet.py --changed-files <file> --risk-report <report> --evidence-manifest <manifest> --head-sha <sha>
```

## Claude Feedback Marker Format

Claude comments/reviews are ingested only when they contain:

- marker: `<!-- openfang-claude-feedback -->`
- fenced JSON block (no heuristic parsing)

Example:

````markdown
<!-- openfang-claude-feedback -->
```json
{
  "head_sha": "abc1234deadbeef...",
  "summary": "Claude review summary",
  "findings": [
    {
      "id": "claude-1",
      "severity": "high",
      "confidence": 0.88,
      "path": "crates/openfang-runtime/src/agent_loop.rs",
      "line": 120,
      "summary": "Guard misses error path finish",
      "actionable": true
    }
  ]
}
```
````

If `requireHeadShaMatch=true`, stale `head_sha` comments are ignored.

`claude-remediation-agent` is always-on for PR syncs and uses:

- `OPENFANG_CLAUDE_REMEDIATION_CMD`
- `OPENFANG_CLAUDE_VALIDATION_CMD`

## Codex Dual-Account Failover

Both remediation workflows use `scripts/harness/codex_failover_runner.py`:

- `.github/workflows/claude-remediation-agent.yml`
- `.github/workflows/sentry-remediation-agent.yml`

### Required secrets

- `CODEX_AUTH_JSON_B64_PRIMARY` (preferred primary auth payload)
- `CODEX_AUTH_JSON_B64_SECONDARY` (secondary/failover auth payload)

Legacy compatibility is preserved:

- `CODEX_AUTH_JSON_B64` (legacy primary fallback)

### Runtime resolution order

1. Primary auth = `CODEX_AUTH_JSON_B64_PRIMARY`, else `CODEX_AUTH_JSON_B64`.
2. Secondary auth = `CODEX_AUTH_JSON_B64_SECONDARY`.

### Failover policy

Failover is single retry only (`primary -> secondary`) and triggers only for rate/quota signatures:

- `You've hit your usage limit`
- `rate limit`
- `429`
- `Too Many Requests`
- `try again at`

Non-rate failures do not retry secondary.

### Failover artifact

Both workflows upload:

- `artifacts/codex-failover-result.json`

This includes:

- `used_primary`, `used_secondary`, `failover_triggered`
- `primary_exit_code`, `secondary_exit_code`, `final_exit_code`
- `final_account` (`primary|secondary|none`)
- `trigger_reason` (`rate_limit|none|secondary_missing`)
- `rate_limit_detected`

### Troubleshooting

| Condition | What it means | Expected behavior |
|---|---|---|
| `trigger_reason=secondary_missing` | Primary looked rate-limited, but no secondary secret configured | Workflow fails after primary, no retry |
| both accounts limited | Primary and secondary both return rate/quota failures | Workflow fails; failover metadata shows both attempts |
| non-rate primary failure | Failure was compile/test/syntax/policy/guardrail, not quota | No secondary retry; fix root cause directly |

## Deterministic Agent Evals

OpenFang now includes a dedicated eval control plane:

- `.github/workflows/infra-preflight.yml` (shared infra/env health check)
- `.github/workflows/agent-evals-pr.yml` (blocking on PRs)
- `.github/workflows/agent-evals-live-pr.yml` (live-provider probes for high/critical PRs)
- `.github/workflows/agent-evals-nightly.yml` (advisory deep sweeps)
- `.github/workflows/agent-evals-backfill.yml` (nightly novel-failure scenario candidate generation)
- `.github/workflows/agent-eval-remediation.yml` (label-gated Codex auto-fix PRs)
- `.harness/evals/scenarios.blocking.json`
- `.harness/evals/scenarios.nightly.json`
- `.harness/evals/scenarios.generated.json` (candidate-only generated corpus)
- `.harness/evals/novel-failure-signatures.json` (dedupe signatures)
- `scripts/harness/agent_eval_runner.py`
- `scripts/harness/agent_eval_judges.py`
- `scripts/harness/agent_eval_fixtures.py`
- `scripts/harness/infra_preflight.py`
- `scripts/harness/live_provider_probe.py`
- `scripts/harness/novel_failure_backfill.py`
- `scripts/harness/infra_incident_upsert.py`

### Policy Contract

`agentEvalPolicy` in `.harness/policy.contract.json` controls:

- `enabled`
- `blockingCheckName` (default `agent-evals-pr`)
- `blockingProfile` (`llm_mode=mock_frozen`, `network_mode=isolated`, `seed=42`)
- `nightlyProfile`
- `blockingThreshold` (set to `1.0` for 100% blocker pass)
- `maxScenarioRuntimeSecs`
- `remediationLabel` (default `eval-remediate`)
- `maxRemediationFindingsPerRun`
- `liveProviderGate`
- `novelFailureBackfill`
- `infraPreflight`

### Live Provider Gate

`agentEvalPolicy.liveProviderGate` controls high/critical live probes:

- `enabled`
- `blockingRiskTiers` (default `critical`, `high`)
- `checkName` (`agent-evals-live-pr`)
- `minSuccessfulProviders`
- `retries` (`attempts`, `backoffSeconds`)
- `providerCatalog` (OpenAI/Anthropic/Gemini via env-based auto-detect)
- `failIfNoProviderSecrets`

Behavior:

- Runs on all PRs.
- Blocks high/critical risk tiers on probe failure.
- Non-blocking tiers remain advisory.

### Infra Preflight and Incident Upsert

`agentEvalPolicy.infraPreflight` controls fail-fast infra checks:

- `enabled`
- `checkName` (`infra-preflight`)
- `requiredForWorkflows`
- `retryPolicy`
- `incident` (`enabled`, `label`, `marker`)

Each integrated workflow runs preflight first and then:

- opens/reopens/updates a sticky infra incident issue when failing
- closes the incident automatically on recovery
- fails the workflow when infra remains unhealthy

### Novel Failure Backfill

`agentEvalPolicy.novelFailureBackfill` controls candidate generation:

- `enabled`
- `maxNewScenariosPerRun`
- `minConfidence`
- `targetScenarioFile`
- `dedupeFile`
- `autoPr`

Nightly backfill mines actionable findings (Sentry/evals/review/Claude), fingerprints novel signatures, and opens/updates PRs with scenario stubs.

### Artifacts

All eval runs produce:

- `artifacts/agent-evals/eval-results.json`
- `artifacts/agent-evals/eval-findings.json`
- `artifacts/agent-evals/eval-summary.md`
- `artifacts/agent-evals/eval-metrics.json`
- `artifacts/agent-evals/live-provider-report.json` (live-provider lane)
- `artifacts/infra-preflight-report.json`
- `artifacts/infra-incident-upsert.json`
- `artifacts/agent-evals/novel-failure-candidates.json` (backfill workflow)

`eval-findings.json` is normalized to remediation format with `provider: "eval"`.

### Scenario Authoring Rules

Each scenario must include:

- `id`, `name`, `tier`, `surface`
- `setup`, `stimulus`, `expected`
- `failure_class`, `remediable`, `owner`, `timeout_secs`

## Failure Visibility to Sentry

Workflow-level failure telemetry now uses:

- `scripts/harness/workflow_sentry_emit.py`
- `scripts/harness/sentry_client.py`
- `.harness/schemas/sentry-workflow-emit.schema.json`
- `.github/workflows/sentry-workflow-telemetry-smoke.yml` (manual smoke for all reason codes)

### Contract settings

`sentryWorkflowTelemetry` in `.harness/policy.contract.json`:

- `enabled`
- `emitOnFailureOnly`
- `dsnEnv` (default `SENTRY_DSN`)
- `artifactPath` (default `artifacts/sentry-workflow-emit.json`)

Repository-level toggle:

- `OPENFANG_SENTRY_WORKFLOW_EVENTS=true` enables workflow event emission.

### Coverage matrix

| Workflow | Emits from | Expected reason codes |
|---|---|---|
| `risk-policy-gate` | gate job terminal step | `infra_preflight_failed`, `live_provider_gate_failed`, `risk_policy_failed`, `findings_ingest_failed`, `workflow_failed_unknown` |
| `pr-review-harness` | `enforce` job terminal step | `infra_preflight_failed`, `risk_policy_failed`, `agent_blocking_evals_failed`, `workflow_failed_unknown` |
| `agent-evals-pr` | `agent-evals-pr` job terminal step | `infra_preflight_failed`, `agent_blocking_evals_failed`, `workflow_failed_unknown` |
| `agent-evals-live-pr` | `agent-evals-live-pr` job terminal step | `infra_preflight_failed`, `live_provider_gate_failed`, `workflow_failed_unknown` |
| `infra-preflight` | preflight job terminal step | `infra_preflight_failed`, `workflow_failed_unknown` |
| `sentry-remediation-agent` | terminal steps in main job | `infra_preflight_failed`, `codex_rate_limit_exhausted`, `remediation_failed`, `findings_ingest_failed`, `workflow_failed_unknown` |
| `claude-remediation-agent` | terminal steps in main job | `infra_preflight_failed`, `codex_rate_limit_exhausted`, `remediation_failed`, `findings_ingest_failed`, `workflow_failed_unknown` |
| `agent-eval-remediation` | terminal steps in main job | `infra_preflight_failed`, `codex_rate_limit_exhausted`, `remediation_failed`, `findings_ingest_failed`, `workflow_failed_unknown` |
| `agent-evals-backfill` | terminal step in main job | `agent_blocking_evals_failed`, `findings_ingest_failed`, `workflow_failed_unknown` |
| `agent-evals-nightly` | terminal step in main job | `agent_blocking_evals_failed`, `workflow_failed_unknown` |

### Advisory emission semantics

- `workflow_sentry_emit.py` always exits `0`.
- Sentry delivery failures (missing DSN, 429, network errors) are recorded in `artifacts/sentry-workflow-emit.json`.
- CI pass/fail remains governed by existing policy checks and enforcement steps.

### Troubleshooting

| Symptom | Meaning | Action |
|---|---|---|
| `sent=false`, detail `missing env SENTRY_DSN` | DSN not available in workflow env | Add/fix `SENTRY_DSN` repo/org secret |
| `sent=false`, detail `http_error=429` | Project ingest rate-limited | Lower event volume or sampling; retry |
| `sent=false`, detail `url_error=...` | Runner cannot reach Sentry ingest host | Check network/DNS and Sentry region host |
| `reason_code=workflow_failed_unknown` | Failure did not match known classifier | Inspect workflow logs/artifacts and extend classifier mapping |

### Manual smoke test in Actions

Run `sentry-workflow-telemetry-smoke` with `workflow_dispatch` to simulate every reason code in one run:

- `infra_preflight_failed`
- `live_provider_gate_failed`
- `risk_policy_failed`
- `agent_blocking_evals_failed`
- `codex_rate_limit_exhausted`
- `remediation_failed`
- `findings_ingest_failed`
- `workflow_failed_unknown`

Inputs:

- `emit_sentry` (`true|false`)
- `require_sentry_delivery` (`true|false`)

`require_sentry_delivery=true` will fail any matrix case where `.sent != true`.

Optional:

- `judge` (`regex_present`, `regex_absent`, `file_exists`, `json_field_exists`, `json_array_min_length`, `json_number_range`, `command_exit`, `fixture_contains`)
- `finding` (`path`, `line` or `line_hint_pattern`, `summary`, `severity`)
- `when_touched` (path filters for changed files)

### Remediation Flow

`agent-eval-remediation` is label-gated and branch-safe:

1. Trigger requires PR label `eval-remediate` (or trusted `workflow_dispatch`).
2. Runs deterministic blocking evals and extracts actionable eval findings.
3. Runs `codex_failover_runner.py` with dual-account fallback.
4. Pushes fixes to bot branch `codex/eval-remediation-pr-<pr_number>`.
5. Opens/updates remediation PR targeting the original PR branch.

No direct push is made to user feature branches.

Defaults are built in:

- If `OPENFANG_EVAL_REMEDIATION_CMD` is unset, workflow uses a safe default Codex command.
- If `OPENFANG_EVAL_VALIDATION_CMD` is unset, workflow defaults to `cargo build --workspace --lib`.

### Sentry Visibility for Eval Infrastructure

`scripts/harness/eval_metrics_emit.py` can emit sanitized synthetic evaluator events to Sentry.

Enable with:

- `OPENFANG_EVAL_EMIT_SENTRY=true` (PR eval workflow)
- `OPENFANG_EVAL_EMIT_SENTRY_NIGHTLY=true` (nightly eval workflow)
- `SENTRY_DSN` secret

Only aggregate counts/tags are sent; no prompt or content payloads are forwarded.

### Failure Triage Matrix

| Signal | Meaning | Action |
|---|---|---|
| `all_blocking_passed=false` in `eval-results.json` | Blocking deterministic scenario failed | Review `eval-summary.md`, patch failing path, rerun PR |
| `provider=eval` findings present | Actionable auto-fix candidates exist | Add `eval-remediate` label to trigger Codex remediation PR |
| `trigger_reason=secondary_missing` | Primary account rate-limited and secondary auth missing | Add `CODEX_AUTH_JSON_B64_SECONDARY` secret |
| `status=error` in `eval-findings.json` | Eval runner/judge execution problem | Fix scenario contract or judge implementation |
| `agent-evals-pr` check missing on high/critical PR | Required eval check not executed | Verify workflow enabled and branch protection required checks |
| `infra-preflight` check failing | Secrets/connectivity/tooling outage in CI env | Inspect `infra-preflight-report.json`, resolve outage, rerun |
| `agent-evals-live-pr` failing for high/critical PR | Live provider probes unhealthy or quota/auth issue | Check `live-provider-report.json`, rotate/replenish provider credentials |
| `new_candidates > 0` in novel backfill report | New failure signatures detected from production findings | Review generated scenario PR, refine/promote candidates |

## Notes

- Gate and fanout are designed to avoid spending CI time on PR heads already blocked by policy.
- Remediation is constrained to contract-allowed paths and forbids control-plane bypass changes.
- Weekly metrics track stale-review rate, rerun pressure, remediation performance, and high-tier pass rate.
- This PR validated required Claude ingestion with always-on remediation.
