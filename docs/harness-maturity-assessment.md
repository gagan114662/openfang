# OpenFang Harness Maturity Assessment

**Date**: 2025-01-15 (updated 2025-05-20)
**Scope**: Anthropic "Agentic Coding" article pattern coverage

## Pattern Coverage Summary

| # | Pattern | Score | Notes |
|---|---------|-------|-------|
| 1 | Context folding via sub-LLM | 4/5 | `context_folder.rs` — UTF-8 panic fixed |
| 2 | FEATURES.json + PROGRESS.md | 4/5 | `feature_list.rs`, `progress.rs` — parsers complete |
| 3 | Browser-based feature verification | 3/5 | `browser_automation.rs` — assertion engine + plan executor |
| 4 | Agent-friendly linter messages | 4/5 | `arch_enforcement.rs` — LayerViolation with remediation steps |
| 5 | Startup sequence protocol | 3/5 | `startup_sequence.rs` — 5-step canonical sequence |
| 6 | Observability hooks | 3/5 | `observability.rs` — log querying for agents |
| 7 | Initializer agent | 3/5 | `agents/initializer/agent.toml` — first-session bootstrap |
| 8 | Delegation detection | 4/5 | `delegation.rs` — pluggable detector registry |

**Overall: 28/40**

## Layer Architecture

### Layer 0 — Types
| Module | Status |
|--------|--------|
| `openfang-types` | Stable |

### Layer 1 — Memory
| Module | Status |
|--------|--------|
| `openfang-memory` | Stable |

### Layer 2 — Runtime
| Module | Status |
|--------|--------|
| `openfang-runtime` | Stable |
| `context_folder.rs` | UTF-8 fix applied |
| `delegation.rs` | Complete |
| `feature_list.rs` | Complete |
| `progress.rs` | Complete |
| `browser_automation.rs` | New — assertion engine |
| `startup_sequence.rs` | New — 5-step protocol |
| `observability.rs` | New — log querying |
| `browser.rs` | Stable — Playwright bridge |

### Layer 2 — Agents
| Agent | Status |
|-------|--------|
| `initializer` | New — first-session bootstrap |
| `coder`, `analyst`, `debugger`, etc. | Stable (31 agents) |

### Layer 3+ — Kernel, API, Frontends
No harness-specific changes at these layers.

## Rust Test Coverage

| Module | Test Count | Coverage |
|--------|-----------|----------|
| `context_folder` | 7 | Threshold, defaults, UTF-8 boundary, multibyte truncation |
| `browser_automation` | 12 | All assertion types, plan construction, verify, report |
| `startup_sequence` | 9 | Step ordering, pass/fail, orientation context |
| `observability` | 14 | Log parsing, filtering, errors, section formatting |
| `delegation` | 8+ | Detector registry, confidence ranking |
| `feature_list` | 6+ | JSON parsing, status tracking |
| `progress` | 6+ | GFM task list parsing |
| `arch_enforcement` | 3 | Layer violations, circular deps, remediation format |

## Gap Analysis

### Completed
- UTF-8 slicing panic in `truncate_fallback()` — `floor_char_boundary()` helper
- Agent-friendly violation messages with structured remediation steps
- Browser automation assertion engine with plan/verify/report cycle
- Startup sequence protocol with orientation context builder
- Observability log querying for agent self-diagnosis
- Initializer agent for first-session bootstrap

### Remaining Gaps
- Browser automation does not yet drive live Playwright sessions (uses assertion-only path)
- Observability does not yet integrate with tracing subscriber for real-time log tailing
- Startup sequence needs kernel-level wiring to execute at agent session start
- No integration test for full init → feature-list → progress → verify cycle
- `context_folder.rs` fold path not tested e2e (requires mock LLM driver)

## Methodology

Assessment based on the Anthropic "Tips for building agentic coding tools" article patterns:
1. **Context management** — sub-LLM folding, context budget, overflow detection
2. **Feature tracking** — structured FEATURES.json + PROGRESS.md for cross-session state
3. **Verification** — browser-based e2e checks, not just unit tests
4. **Developer experience** — agent-friendly error messages with actionable remediation
5. **Startup protocol** — deterministic orientation sequence for new sessions
6. **Observability** — agents can query their own logs for self-diagnosis
7. **Bootstrap** — dedicated initializer agent for first-session setup
8. **Delegation** — pluggable detection for routing to specialized sub-agents
