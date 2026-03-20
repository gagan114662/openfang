# Harness Maturity Assessment

Assessment of OpenFang's agent harness infrastructure against the five
design patterns identified in the harness engineering literature
(SWE-agent ACI, Anthropic two-agent architecture, OpenAI Codex
zero-manual-code) and the seven-layer taxonomy from the
awesome-agent-harness ecosystem map.

**Date**: 2026-03-19
**Rollout phase**: phase-2 (Strict Claude+Codex pre-approval loop)
**Asset count**: ~110 files (scripts, workflows, schemas, configs, runtime modules)

---

## Pattern Scorecard

| # | Pattern | Status | Score | Evidence |
|---|---------|--------|-------|----------|
| 1 | Progressive Disclosure | Implemented | 9/10 | Short CLAUDE.md map + deep docs/; capped context_budget (30% per-result, 75% headroom); startup orientation sequence in worktree scripts |
| 2 | Git Worktree Isolation | Implemented | 10/10 | Root read-only; `claude/<task>` and `codex/<task>` branches; concurrent lock system; `of-claude`/`of-codex` launchers; clean-at-rest enforcement |
| 3 | Spec First / Repo as System of Record | Implemented | 8/10 | `policy.contract.json` drives all enforcement; JSON schemas validate artifacts; feature_list.rs + progress.rs parse FEATURES.json and PROGRESS.md. Gap: no automated feature-list generation (initializer agent pattern not yet wired end-to-end) |
| 4 | Mechanical Architecture Enforcement | Implemented | 9/10 | `xtask check-layers` validates crate dependency hierarchy; risk-policy-gate blocks merge mechanically; CI fanout per risk tier; remediation constrained to allowed paths. Gap: no custom linter generating agent-targeted remediation messages (OpenAI pattern) |
| 5 | Integrated Feedback Loops | Implemented | 8/10 | Claude hooks emit structured Sentry events; risk-policy-gate returns actionable decisions; remediation runner applies fixes inline; weekly metrics aggregation. Gap: no browser automation (Puppeteer MCP) for end-to-end UI verification |

**Overall: 44/50 (88%)**

---

## Seven-Layer Coverage

### Layer 1: Human Oversight

| Component | Status | Implementation |
|-----------|--------|----------------|
| PR approval gate | Done | `risk-policy-gate` + `pr-review-harness` workflows |
| Risk tier visibility | Done | Report artifact with tier, decision, reasons |
| Rollout phase control | Done | phase-0/1/2 in `policy.contract.json` |
| Manual override | Done | `root_mode.sh unlock` for root checkout |

### Layer 2: Planning and Requirements (Spec Tools)

| Component | Status | Implementation |
|-----------|--------|----------------|
| Machine-readable policy | Done | `.harness/policy.contract.json` (210 lines) |
| JSON schemas | Done | 3 schemas in `.harness/schemas/` |
| Feature tracking | Done | `feature_list.rs` parses FEATURES.json (pass/fail/blocked) |
| Progress tracking | Done | `progress.rs` parses PROGRESS.md (GFM task lists) |
| Automated spec generation | Gap | No initializer agent that auto-generates feature list from prompt |

### Layer 3: Full Lifecycle Platforms

| Component | Status | Implementation |
|-----------|--------|----------------|
| Deterministic PR sequence | Done | 10-step ordered flow (gate -> fanout -> remediation -> review -> sentry) |
| Artifact pipeline | Done | risk-policy-report.json, review-findings.json, claude-findings.json, remediation-result.json, browser-evidence-manifest.json |
| Telemetry | Done | `emit_structured_event.py` posts lifecycle events to daemon + Sentry |
| Weekly metrics | Done | `harness-weekly-metrics.yml` aggregates stale-review rate, rerun pressure, remediation success |

### Layer 4: Task Runners

| Component | Status | Implementation |
|-----------|--------|----------------|
| Claude remediation agent | Done | `claude-remediation-agent.yml` - auto-applies fixes for actionable findings |
| Sentry remediation agent | Done | `sentry-remediation-agent.yml` - scheduled/manual issue remediation |
| CI fanout | Done | `ci-fanout.yml` - runs only required checks per risk tier |
| One-attempt-per-SHA guard | Done | Prevents redundant remediation via git commit marker |

### Layer 5: Agent Orchestrators

| Component | Status | Implementation |
|-----------|--------|----------------|
| Worktree isolation | Done | 10 scripts in `scripts/worktree/` |
| Concurrent lock system | Done | Prevents Claude + Codex on same worktree |
| Branch routing | Done | `claude/<task>` and `codex/<task>` convention |
| Clean-at-rest enforcement | Done | `finish_agent_task.sh` validates cargo build + test before session end |
| Multi-agent parallel execution | Partial | Worktree infrastructure supports it; no orchestrator that auto-spawns parallel agents from a task DAG |

### Layer 6: Agent Harness Frameworks and Runtimes

| Component | Status | Implementation |
|-----------|--------|----------------|
| Context budget management | Done | `context_budget.rs` - two-layer system (per-result 30%, context guard 75%) |
| Workspace context aggregation | Done | `workspace_context.rs` + `context_folder.rs` |
| Prompt builder | Done | `prompt_builder.rs` + PromptContext in kernel.rs (two construction sites) |
| Delegation framework | WIP | `delegation.rs` - skeleton for inter-agent routing |
| Identity/config loading | Done | `read_identity_file()` pattern in kernel |
| Claude Desktop hooks | Done | `claude_hook.py` - lifecycle events, branch guards, audit trail |
| MCP server integration | Done | `.mcp.json` exposes `openfang` + `contextplus` servers |

### Layer 7: Coding Agents

| Component | Status | Implementation |
|-----------|--------|----------------|
| Agent templates | Done | 35+ pre-built agents in `agents/` directory |
| Claude Code integration | Done | CLAUDE.md instructions, hooks, MCP, worktree launchers |
| Codex integration | Done | AGENTS.md instructions, worktree launchers |
| Runtime agent loop | Done | `crates/openfang-runtime/src/agent_loop.rs` |
| Model orchestrator | Done | Multi-provider routing (Anthropic, OpenAI, Gemini, Ollama) |

---

## Gap Analysis

### High Priority (blocks next phase of harness maturity)

| Gap | Pattern Reference | Impact | Suggested Fix |
|-----|-------------------|--------|---------------|
| No browser automation | Anthropic Pattern: Puppeteer MCP for e2e testing | Agents mark features complete without verifying UI works end-to-end | Add Puppeteer/Playwright MCP server to `.mcp.json`; wire into `pr-review-harness` for UI-impacting paths |
| No initializer agent | Anthropic Pattern: Two-agent architecture | Feature lists and progress files are manually created, not auto-generated from a prompt | Build an `init` xtask command or agent template that generates FEATURES.json + init.sh + PROGRESS.md |

### Medium Priority (improves reliability and throughput)

| Gap | Pattern Reference | Impact | Suggested Fix |
|-----|-------------------|--------|---------------|
| No agent-targeted linter messages | OpenAI Pattern: Linters with remediation instructions | When `check-layers` fails, error message is human-oriented, not agent-optimized | Add `--agent-format` flag to xtask linters that outputs JSON with violation + rule + remediation steps |
| delegation.rs is a skeleton | Anthropic Pattern: Multi-session handoff | No automated inter-agent task routing at runtime | Complete delegation module with task queue, capability matching, handoff protocol |
| No parallel task DAG orchestrator | OpenAI Pattern: Parallel agent execution | Worktree infra exists but no tool auto-spawns agents from a decomposed task | Build orchestrator that reads task DAG, spawns worktrees, assigns agents, merges results |

### Low Priority (polish and observability)

| Gap | Pattern Reference | Impact | Suggested Fix |
|-----|-------------------|--------|---------------|
| pr_packet.py is mostly stubbed | SWE-agent Pattern: Integrated feedback | Acceptance checklists lack structured execution evidence | Flesh out `build_pr_packet()` with real evidence collection |
| No context compaction metrics | SWE-agent Pattern: Context management | Cannot measure how often agents hit context limits or lose coherence | Add telemetry counters for compaction events in agent_loop.rs |
| Weekly metrics are basic | OpenAI Pattern: Throughput tracking | No per-engineer PR velocity or per-tier pass rate trends | Extend `harness-weekly-metrics.yml` with time-series output |

---

## What Is Working Well

1. **Policy contract is the single source of truth.** All 10 workflows, 16 scripts, and the runtime modules read from `policy.contract.json`. No hardcoded enforcement logic.

2. **Current-head SHA discipline prevents stale reviews.** The gate rejects reviews from old commits, deduplicates rerun requests per SHA, and limits remediation to one attempt per SHA.

3. **Worktree isolation is fully enforced.** Root checkout is read-only by default. Agents cannot run outside their designated worktree. Sessions refuse to end dirty.

4. **Feedback loops are closed at multiple levels.** Claude hooks post structured events to Sentry. The risk-policy-gate returns machine-readable decisions. Remediation applies fixes inline. Weekly metrics track drift.

5. **Phased rollout allows gradual enforcement.** Phase-0 (advisory) through phase-2 (strict enforcement) lets the team ratchet up safety as confidence grows.

6. **Context budget prevents overflow.** The two-layer system (30% per-result cap, 75% headroom guard) directly addresses the SWE-agent finding that context flooding is the primary agent failure mode.

---

## Comparison to Reference Implementations

### vs. SWE-agent ACI (Princeton NLP)

| ACI Component | SWE-agent | OpenFang | Match |
|---------------|-----------|----------|-------|
| Capped search results | 50-result limit | context_budget 30% per-result cap | Equivalent |
| Stateful file viewer with line numbers | 100-line window, line numbers prepended | Read tool with line numbers (Claude Code native) | Equivalent |
| File editor with linting | Edit + auto-lint, reject on syntax error | Edit tool + `cargo check` in finish gate | Partial (no per-edit lint) |
| Context compression | Collapse old observations to summaries | context_budget headroom guard + compaction | Equivalent |

### vs. Anthropic Two-Agent Architecture

| Component | Anthropic | OpenFang | Match |
|-----------|-----------|----------|-------|
| Initializer agent | Dedicated first session: init.sh, feature list, progress file | Manual setup, but runtime modules parse FEATURES.json + PROGRESS.md | Partial |
| Feature list as JSON | 200+ features, passes field | `feature_list.rs` supports same schema | Ready |
| Progress tracking | claude-progress.txt updated each session | `progress.rs` parses PROGRESS.md task lists | Ready |
| Clean state requirement | Git commit + progress update at session end | `finish_agent_task.sh` enforces build + test + no dirty files | Full match |
| Startup sequence | pwd -> progress -> features -> init.sh -> verify | Worktree scripts orient agent on entry | Partial |
| Browser testing via Puppeteer | Puppeteer MCP for e2e verification | Not yet wired | Gap |

### vs. OpenAI Codex Zero-Manual-Code

| Component | OpenAI | OpenFang | Match |
|-----------|--------|----------|-------|
| Repository as system of record | Structured docs/ + short AGENTS.md map | docs/ (20+ files) + CLAUDE.md/AGENTS.md as maps | Full match |
| Mechanical architecture enforcement | Custom linters with agent-targeted error messages | `xtask check-layers` + risk-policy-gate | Partial (messages not agent-optimized) |
| Worktree-per-task isolation | Per-task git worktrees | `of-claude`/`of-codex` worktree launchers | Full match |
| Application legibility (observability) | Chrome DevTools Protocol + LogQL/PromQL/TraceQL | Sentry integration + structured events | Partial (no local observability stack) |
| Throughput-adjusted merge philosophy | Minimal blocking gates, short-lived PRs | Phase-controlled enforcement, CI fanout by risk tier | Full match |
| Golden principles via recurring cleanup | Background tasks scan for deviations | Weekly metrics workflow | Partial |

---

## Maturity Level

Based on the five-level maturity model (ad hoc -> repeatable -> defined -> managed -> optimizing):

| Dimension | Level | Evidence |
|-----------|-------|----------|
| Policy & Contracts | **Managed** | Single contract drives all enforcement; phased rollout; schemas validate artifacts |
| Worktree Isolation | **Optimizing** | Fully enforced; concurrent locks; auto-route launchers; clean-at-rest |
| Feedback Loops | **Managed** | Sentry events; risk-gate decisions; remediation; weekly metrics |
| Context Management | **Defined** | Budget caps; headroom guards; prompt builder. No compaction metrics yet |
| Spec / Feature Tracking | **Defined** | Runtime modules ready; not yet wired to automated initializer |
| Agent Orchestration | **Repeatable** | Manual worktree creation; no auto-spawn from task DAG |
| Browser / E2E Testing | **Ad hoc** | Evidence schema exists; no automation wired |

**Overall maturity: Defined to Managed (Level 3-4 of 5)**

The infrastructure is production-ready for single-agent workflows with
mechanical enforcement. The primary gaps are in multi-agent orchestration
and automated end-to-end verification — both of which have the
infrastructure foundations already in place (worktree isolation, evidence
schemas, delegation skeleton) but lack the wiring to run autonomously.
