# YC Next Actions

Date: 2026-03-05
Owner: Codex automation + manual sessions
Status: active

## Rules

- Work top to bottom.
- Prefer the smallest task that materially improves the unattended demo or the YC story.
- Update this file after every completed automation cycle.
- If blocked, add a blocker note directly under the item.

## Priority 0

- [ ] Complete remote cutover so the GPU host is the sole primary runtime and Telegram poller.
- [ ] Verify live `ops.guard.*` logs in Sentry from the active primary host.
- [ ] Make one exact Sentry query per critical event kind easy to reuse from the runbook.

## Priority 1

- [ ] Close the loop from detected auth failure to Telegram-delivered intervention request.
- [ ] Integrate provider quarantine/reroute into runtime failure handling.
- [ ] Reduce "Agent is unresponsive" log floods with dedupe or backoff.

## Priority 2

- [ ] Produce a single crisp YC wedge statement and product one-liner.
- [ ] Write a 60-90 second demo script around unattended operation + Telegram recovery + Sentry visibility.
- [ ] Create a concise architecture diagram and operator workflow doc.
- [ ] Draft YC application answers in `docs/yc/application-draft.md`.

## Priority 3

- [ ] Add an investor-facing landing-page narrative for the wedge.
- [ ] Create a proof log of successful unattended runs and remediations.
- [ ] Add a daily operating brief generated from guard artifacts and Sentry-visible state.
