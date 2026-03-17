<!-- pr-review-checklist:start -->
## PR Review Harness Checklist

- Head SHA: `local-dev-sha`
- Overall status: FAIL

### Acceptance Criteria
- [x] **Diff is scoped and coherent** (`diff_scoped_coherent`)
  - 2 changed files within maxFiles=200
- [x] **Required CI signals are green** (`required_ci_signals_green`)
  - no non-harness required CI checks declared for this risk tier
- [x] **Evidence package exists** (`evidence_package_exists`)
  - manifest contains 5 artifacts
- [x] **At least 2 screenshots captured** (`minimum_screenshots`)
  - 3 screenshots >= required 2
- [x] **At least 1 video captured** (`minimum_videos`)
  - 1 videos >= required 1
- [ ] **No harness policy violations** (`no_harness_policy_violations`)
  - risk gate decision: unknown
- [x] **Agent blocking evals pass** (`agent_blocking_evals_pass`)
  - agent-evals-pr not required for this risk tier
- [ ] **api_runtime_validation_present** (`api_runtime_validation_present`)
  - required checks missing for API/runtime scope: ci-check, ci-test

### Review Providers

- Primary provider: `greptile`
- Greptile check run: `greptile-review`
- Greptile report state: `missing`
- Greptile check-run state: `missing/n/a`

### Agent Eval Summary

- Blocking check: `agent-evals-pr`
- Check status: `missing/n/a`
- Eval totals: `34/34` passed; all_blocking_passed=`true`

### Evidence Inventory

| Type | Path | Size (bytes) | SHA256 |
| --- | --- | ---: | --- |
| screenshot | `pr-review/evidence/01-diff-summary.png` | 35999 | `fb22adaa762c59c1cb8770fac2eabee98311924c04da1bea18d5dbad32fa3c60` |
| screenshot | `pr-review/evidence/02-verification-summary.png` | 23462 | `0f9f1e3245be407bcee1eb89a73f8ec16463e8b9046e5fe594b0d26773155138` |
| screenshot | `pr-review/evidence/03-checklist-preview.png` | 32678 | `33c3f8905f3d6e193e886099f8cdd0b666c471a2be3cc3c4269caddf335571dc` |
| video | `pr-review/evidence/00-implementation-walkthrough.mp4` | 115187 | `b530e23ed28dfa1f19e5052a46378646a76e1d8bf1206413229b9a7b21086a09` |
| log | `context/changed_files.txt` | 76 | `5595dbc837f2fe237e4a081fa5482320ed9827aa9b51442bfd1af21fe1aead2e` |
<!-- pr-review-checklist:end -->
