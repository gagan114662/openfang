# OpenFang Funding Thesis

Date: 2026-03-05
Status: draft

## Current Working Thesis

OpenFang is an unattended agent operations system:
- agents run on remote compute
- humans intervene through Telegram only when needed
- Sentry is the operational truth layer
- the system is observable, recoverable, and controllable without sitting in front of a terminal

## Why This Matters

Most "AI agents" fail in production for operational reasons:
- no clear control plane
- weak observability
- no reliable human escalation path
- auth/session failures stop work entirely
- duplicate processes and hidden state make recovery brittle

OpenFang's wedge is not generic "AI agents."
It is: reliable unattended agent operation with human-in-the-loop recovery over channels people already use.

## Current Demo Shape

The strongest near-term demo is:
- a remote-first OpenFang runtime on the GPU host
- Telegram as the human control and escalation surface
- Sentry showing canonical `api.request`, `runtime.*`, `ops.guard.*`, and `auth.*` logs
- guard/remediation loops stopping duplicate pollers and restarting unhealthy services
- auth recovery via Telegram commands instead of shell access

## What Needs To Be True For YC

1. The wedge is narrow and legible.
2. The demo works end-to-end without handholding.
3. The product feels operationally credible, not aspirational.
4. The repo and docs show a repeatable system, not a one-off hack.

## Constraints

Do not drift into:
- broad "agent operating system" marketing
- too many product surfaces at once
- speculative monetization ideas without a working wedge
- features that do not improve demo reliability or fundability

## Next Work

Source of truth for execution order:
- `docs/yc/next-actions.md`
