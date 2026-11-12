---
title: 'pr-router-stale-drainer: surface a stale hook consumer in status and warn once per episode'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-12T13:30:00.000000000+00:00
updated_at: 2026-11-12T13:30:00.000000000+00:00
closed_at: 2026-11-12T13:30:00.000000000+00:00
---

# Description

A downstream consumer that drains hook events stopped running, and
hundreds of hook events piled up silently: nothing in the queue could
see that the consumer was gone, because the queue never knew the
consumer existed. The consumer-side heartbeat writer and its own loud
warnings live with the consumer; the queue's generic part is a liveness
declaration it can read and report.

# Design

- `hooks.consumer_heartbeat` (path, relative to the state directory)
  and `hooks.consumer_max_age_seconds` (0 = 300) declare the consumer:
  it touches the file while it drains; the queue only reads it.
- `consumer_health` in hooks.rs reports fresh, stale (heartbeat age
  plus hook events emitted after it; a never-touched heartbeat counts
  every logged event), or not declared.
- `merge-queue status` (text and JSON via `StatusReport.hook_consumer`)
  reports the verdict.
- When a hook fires while the consumer is stale, the queue writes one
  warning line to `hooks.jsonl` and stderr per stale episode (keyed by
  the heartbeat value, so a new touch starts a new episode), never one
  per event.

# Acceptance Criteria

- Fresh, stale-with-events, stale-with-no-events, never-touched, and
  not-declared cases are covered
  (`consumer_health_covers_fresh_stale_and_undeclared`).
- Two hooks fired while stale produce exactly one warning line
  (`a_stale_consumer_warns_once_per_episode`).
- `status` reports STALE with the event count in text (and the same
  fields in JSON) (`status_reports_a_stale_hook_consumer`).

# Notes

## Resolution

[watermelon-full-preview] Implemented as designed in state.rs
(`HooksConfig` fields), hooks.rs (`consumer_health`,
`warn_consumer_stale_once`, fired from `fire`), and serve.rs
(`StatusReport.hook_consumer`, rendered line). Tests above pass;
landing follows the usual validation plus gate.
