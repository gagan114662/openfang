# OpenFang Agent Evals

- Head SHA: `local-dev-sha`
- Profile: `blocking`
- Seed: `42`
- Total: `34`
- Passed: `34`
- Failed: `0`
- Pass rate: `1.000`
- Blocking threshold: `1.000`
- All blocking passed: `True`

## Failure Classes

- none

## Scenario Results

| Scenario | Surface | Failure Class | Pass | Duration (ms) | Reason |
| --- | --- | --- | --- | ---: | --- |
| api-integration-test-exists | api | routing_correctness | PASS | 0 | - |
| channels-bridge-integration-test-exists | channels | routing_correctness | PASS | 0 | - |
| checks-resolver-maps-agent-evals | harness | tool_guardrails | PASS | 0 | - |
| checks-resolver-maps-ci-check | harness | tool_guardrails | PASS | 0 | - |
| claude-workflow-label-gate-present | harness | tool_guardrails | PASS | 0 | - |
| codex-failover-rate-limit-signatures | harness | retry_timeout_handling | PASS | 0 | - |
| codex-failover-secondary-trigger | harness | retry_timeout_handling | PASS | 0 | - |
| frozen-llm-routing-fixture | runtime | routing_correctness | PASS | 0 | - |
| frozen-llm-sentry-fixture | runtime | sentry_redaction_invariants | PASS | 0 | - |
| kernel-workflow-integration-test-exists | kernel | memory_isolation | PASS | 0 | - |
| policy-allowed-paths-configured | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| policy-blocked-paths-configured | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| policy-has-remediation-guardrails | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| policy-max-files-guardrail | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| policy-max-lines-guardrail | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| pr-harness-screenshot-minimum-enforced | harness | tool_guardrails | PASS | 0 | - |
| pr-harness-video-minimum-enforced | harness | tool_guardrails | PASS | 0 | - |
| pr-review-harness-workflow-exists | harness | tool_guardrails | PASS | 0 | - |
| remediation-runner-supports-eval-provider | harness | remediation_guardrail_enforcement | PASS | 0 | - |
| risk-gate-loads-claude-findings | harness | tool_guardrails | PASS | 0 | - |
| risk-policy-gate-workflow-exists | harness | tool_guardrails | PASS | 0 | - |
| routing-bridge-resolves-target-agent-name | channels | routing_correctness | PASS | 0 | - |
| routing-email-target-agent-metadata | channels | routing_correctness | PASS | 0 | - |
| routing-router-system-default-support | channels | routing_correctness | PASS | 0 | - |
| routing-router-user-default-support | channels | routing_correctness | PASS | 0 | - |
| sentry-capture-rate-limited | runtime | retry_timeout_handling | PASS | 0 | - |
| sentry-no-raw-error-extra | runtime | sentry_redaction_invariants | PASS | 0 | - |
| sentry-sanitize-helper-exists | runtime | sentry_redaction_invariants | PASS | 0 | - |
| sentry-scope-span-cleared | runtime | retry_timeout_handling | PASS | 0 | - |
| sentry-transaction-finish-called | runtime | retry_timeout_handling | PASS | 0 | - |
| sentry-workflow-uses-codex-failover | harness | tool_guardrails | PASS | 0 | - |
| webhook-agent-token-validation | api | webhook_auth_rejection | PASS | 0 | - |
| webhook-constant-time-compare | api | webhook_auth_rejection | PASS | 0 | - |
| webhook-wake-token-validation | api | webhook_auth_rejection | PASS | 0 | - |
