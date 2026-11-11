---
title: 'chat-latency-headless-accepted: record when the headless wrapper accepts a request, so ingestion compares across paths'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-11-11T16:01:51+00:00
updated_at: 2026-11-11T16:01:51+00:00
---

# Description

[opus 5.5] `agentctl chat latency` reports admitted -> delivered as ingestion, but "delivered" means different things on the two delivery paths:

- for a terminal (TUI) agent, the prompt is typed into the pane, even mid-turn;
- for a headless agent, the model started the turn, which includes the wait behind a busy turn.

A 2026-11-11 comparison of a headless coordinator with terminal coordinators on the same model (requested by the owner) couldn't compare ingestion for this reason. The headless value (p50 9 s) mostly measured the queue wait.

# Design

- The headless wrapper records an "accepted" timestamp when it appends the request to its queue, carried in the bridge's latency record like the existing stage times.
- The report labels ingestion per path: terminal admitted -> typed; headless admitted -> accepted, plus accepted -> turn start shown as queue wait. In `--json`, keep the existing fields and add the new stage.
- `--by` summaries fold the new stage, so per-adapter comparisons line up.
- Records from before the change report the headless stage as absent, never zero.

# Acceptance Criteria

- A headless fixture request carries an accepted time, and the report shows admitted -> accepted and accepted -> turn start separately from admitted -> delivered.
- A terminal fixture request reports admitted -> typed under the same ingestion column.
- Tests use recorded timestamps, not the wall clock.

# Notes

The lead re-runs the headless versus terminal comparison on 2026-11-18, with a week of #1216 agentctl-effort-option effort and model stamps.
