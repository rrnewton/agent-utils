---
title: 'chat-latency-headless-accepted: record when the headless wrapper accepts a request, so ingestion compares across paths'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-11-11T16:01:51+00:00
updated_at: 2026-11-11T16:01:51+00:00
closed_at: 2026-11-11T16:01:51+00:00
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

## Resolution

[watermelon-full-preview] Delivered with this closing commit. A
headless request now carries `accepted_at_millis`: when delivery is
stamped for a headless target, the bridge reads the `queued_at` of
the request's `chat-<key>` queue item (in whichever queue phase
holds it) and records it beside `delivered_at_millis`; terminal
targets and older records leave it absent, never zero. `chat
latency` gains `admitted_to_accepted` (headless ingestion) and
`accepted_to_delivered` ("accepted -> turn start (queue wait)") in
the text report, the JSON stages and per-request fields, while the
existing fields are kept, and the folded summaries and `--by`
groupings carry both new stages (appended, so old summaries read
unchanged). Tests use recorded timestamps: a headless fixture whose
queue item records `queued_at` 1700.25 s stamps accepted 1700250 ms
beside delivery, and the report-level stages split a headless
request's 800 ms ingestion from its 8.2 s queue wait while a
terminal request reports only admitted -> delivered. The Python
edition has no chat latency path to refuse; the wrapper itself is
unchanged - the stamp is read from its queue item. Unverified: no
live headless coordinator was compared; the 2026-11-18 re-run is the
lead's.
