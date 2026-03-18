---
tracker:
  kind: linear
  api_key: $LINEAR_API_KEY
  project_slug: "auto-c7d689304042"
  assignee: $LINEAR_ASSIGNEE
  active_states:
    - Todo
    - In Progress
    - Human Review
    - Merging
    - Rework
  terminal_states:
    - Closed
    - Cancelled
    - Canceled
    - Duplicate
    - Done
polling:
  interval_ms: 5000
workspace:
  root: ~/.openfang/symphony-workspaces/openfang
hooks:
  after_create: |
    git clone --depth 1 https://github.com/gagan114662/openfang.git .
    if command -v cargo >/dev/null 2>&1; then
      cargo fetch || true
    fi
  before_remove: |
    ./scripts/symphony/before_remove.sh
agent:
  max_concurrent_agents: 6
  max_turns: 20
  max_concurrent_agents_by_state:
    Human Review: 1
    Merging: 1
codex:
  command: codex --config shell_environment_policy.inherit=all --config model_reasoning_effort=xhigh app-server
  approval_policy: never
  thread_sandbox: workspace-write
  turn_sandbox_policy:
    type: workspaceWrite
---

You are working on a Linear ticket `{{ issue.identifier }}` for the OpenFang repository.

{% if attempt %}
Continuation context:

- This is retry attempt #{{ attempt }} because the ticket is still in an active state.
- Resume from the existing workspace state instead of restarting from scratch.
- Re-verify only what changed after the previous attempt.
{% endif %}

Issue context:
Identifier: {{ issue.identifier }}
Title: {{ issue.title }}
Current status: {{ issue.state }}
Labels: {{ issue.labels }}
URL: {{ issue.url }}

Description:
{% if issue.description %}
{{ issue.description }}
{% else %}
No description provided.
{% endif %}

Operating rules:

1. This is an unattended Symphony orchestration run. Do not ask a human to do follow-up work.
2. Work only inside the provided Symphony workspace clone for this issue. Never touch `/Users/gaganarora/Desktop/my projects/open_fang` or any sibling checkout.
3. Start by reading `AGENTS.md`, `CLAUDE.md`, and `docs/harness-engineering.md` in the repo. Follow the stricter instruction whenever they differ.
4. Keep one persistent `## Codex Workpad` comment in Linear and update it in place.
5. Before any code edits, capture a concrete reproduction signal and record it in the workpad.
6. Run the repo's required validation for the scope before every push. For code-bearing OpenFang work this includes `cargo build --workspace --lib` and `cargo test --workspace`, plus any ticket-specific validation and live checks required by `AGENTS.md`.
7. For PR handoff, require green current-head checks for `risk-policy-gate`, `claude-remediation-agent`, and `pr-review-harness`, plus any additional checks required by `.harness/policy.contract.json`.
8. Do not move a ticket to `Human Review` until the PR has acceptance checklist markers, execution evidence markers, and no unresolved actionable review feedback.
9. If a task touches app behavior or browser-visible flows, capture runtime/browser evidence and ensure the PR includes the published markers from `pr-review-harness`.
10. If blocked by missing non-GitHub auth or tools, record the blocker concisely in the workpad and move the issue according to the workflow; otherwise continue autonomously.

Workflow state map:

- `Todo` -> move to `In Progress`, create/update the single workpad comment, then execute.
- `In Progress` -> continue implementation and validation.
- `Human Review` -> do not code; poll PR/review state only.
- `Merging` -> land the approved PR and move the issue to `Done`.
- `Rework` -> address feedback, rerun validation, and return to `Human Review` only when green again.
- `Done` -> stop.

Execution requirements:

- Keep the workpad current with plan, acceptance criteria, validation, notes, and confusions.
- Treat all actionable PR comments, review threads, Claude findings, and required checks as blocking until resolved or explicitly pushed back with justification.
- Attach the PR to the Linear issue and ensure the PR has the `symphony` label.
- Keep branch history and PR body clean enough for human approval without additional cleanup.
- Final message must report completed actions and blockers only.
