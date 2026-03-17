# Lint Journal

Track recurring code review issues. When tally reaches 3+, automate detection.

## Automated

| # | Bug Class | Tally | Detection | Status |
|---|-----------|-------|-----------|--------|
| 1 | Provider list in error message drifts from `known_providers()` | 3 | Structural: error message generated from `known_providers().join(", ")` | Eliminated |
| 2 | `known_providers()` / `provider_defaults()` / `builtin_providers()` / `create_driver()` out of sync | 4 | `cargo test`: `test_known_providers_covers_defaults`, `test_defaults_all_resolve`, `test_catalog_providers_match_known` | Automated |
| 3 | Config type map missing non-String channel fields | 3 | `scripts/check_invariants.sh` Check A — parses config.rs and routes.rs | Automated |

## Pending (tally < 3)

| # | Bug Class | Tally | Notes |
|---|-----------|-------|-------|
| 4 | Route handler defined but not registered in server.rs | 1 | `check_invariants.sh` Check B runs as warning; promote at tally 3 |
| 5 | Dashboard provider dropdown out of sync with API | 1 | Hardcoded in `static/index_body.html:948` |

## How to Use

1. During code review, if you spot a class of bug that has occurred before, increment the tally above or add a new row to "Pending".
2. When tally reaches 3, write a lint rule (test or script) and move to "Automated".
3. Each automated entry links to its detection mechanism — a `#[test]` or a `check_invariants.sh` section.
