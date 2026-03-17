# OpenFang Agent Evals

- Head SHA: `local-nightly-sha`
- Profile: `nightly`
- Seed: `42`
- Total: `14`
- Passed: `14`
- Failed: `0`
- Pass rate: `1.000`
- Blocking threshold: `1.000`
- All blocking passed: `True`

## Failure Classes

- none

## Scenario Results

| Scenario | Surface | Failure Class | Pass | Duration (ms) | Reason |
| --- | --- | --- | --- | ---: | --- |
| nightly-chaos-evidence-capture-script | harness | long_horizon_workflow_drift | PASS | 0 | - |
| nightly-chaos-pr-packet-script | harness | long_horizon_workflow_drift | PASS | 0 | - |
| nightly-claude-automation-configured | harness | long_horizon_workflow_drift | PASS | 0 | - |
| nightly-concurrency-kernel-heartbeat | kernel | concurrency_saturation | PASS | 0 | - |
| nightly-concurrency-runtime-loop | runtime | concurrency_saturation | PASS | 0 | - |
| nightly-fixture-concurrency | runtime | concurrency_saturation | PASS | 0 | - |
| nightly-fixture-external | harness | flaky_external_probe | PASS | 0 | - |
| nightly-fixture-provider-fallback | runtime | provider_fallback_degradation | PASS | 0 | - |
| nightly-flaky-external-sentry-script-exists | harness | flaky_external_probe | PASS | 0 | - |
| nightly-flaky-external-telegram-script-exists | channels | flaky_external_probe | PASS | 0 | - |
| nightly-long-horizon-workflow-loop | kernel | long_horizon_workflow_drift | PASS | 0 | - |
| nightly-provider-fallback-catalog-hooks | runtime | provider_fallback_degradation | PASS | 0 | - |
| nightly-provider-fallback-driver-module | runtime | provider_fallback_degradation | PASS | 0 | - |
| nightly-provider-mode-configured | harness | provider_fallback_degradation | PASS | 0 | - |
