# Symphony Setup For OpenFang

This repo now contains a repo-local Symphony workflow contract at `WORKFLOW.md`.

## What it targets

- Linear project slug: `auto-c7d689304042`
- GitHub repo: `gagan114662/openfang`
- Workspace root: `~/.openfang/symphony-workspaces/openfang`
- Required issue states: `Todo`, `In Progress`, `Human Review`, `Merging`, `Rework`

## Required environment

- `LINEAR_API_KEY`
- `LINEAR_ASSIGNEE` if you want Symphony scoped to one assignee
- Codex auth via your normal local Codex setup
- GitHub auth via `gh auth login` or an equivalent local session

## Run locally

```bash
./scripts/symphony/run_local.sh
```

Optional overrides:

- `SYMPHONY_ELIXIR_ROOT`
- `OPENFANG_SYMPHONY_WORKFLOW_PATH`
- `SYMPHONY_LOGS_ROOT`
- `SYMPHONY_PORT`
- `OPENFANG_GITHUB_REPO`

## Behavior

- Symphony polls Linear for issues in the configured states.
- Each issue gets its own isolated workspace clone.
- PR cleanup on terminal issue states is handled by `scripts/symphony/before_remove.sh`.
- Human handoff is blocked by the OpenFang PR gate stack described in `docs/harness-engineering.md`.
