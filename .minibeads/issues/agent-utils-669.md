---
title: 'watch-notice-delivery: watch-agents notices go undelivered behind a stuck batch, marked reported before delivery, and sent to finished managers'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- watch-agents
created_at: 2026-10-28T18:04:09.716143504+00:00
updated_at: 2026-10-28T18:04:09.716143504+00:00
---

# Description

[opus 5.5] watch-agents notices went undelivered for about 25 hours on a live registry (found by another harness's lane-health lane). Three defects in rs/agentctl/src/agent_watch.rs:

1. update_episode set episode.notified when it pushed the notice onto the internal pending list, before any delivery. The pass report's `reported` and the log's per-episode lines therefore claimed delivery for notices that were never queued for anyone, and nothing showed which notices were still owed.
2. The state held one in-flight batch (`inflight: Option<InflightBatch>`). While that batch failed (queue entry stuck crossing the injection barrier, or its requeue chain poisoned in the failed set) it was retried every pass and every later notice, for every recipient, waited behind it.
3. A notice went to the nearest reachable manager even when that manager had itself finished with a verdict, sat at an approval prompt, was dead, or had a turn stuck with input queued: lanes that would not act on it.

Also: watch.log lines carried no time, so an outage's length could not be reconstructed.

Acceptance: notified only after a confirmed queue; owed notices visible in the report and log; each batch retried on its own exponential backoff without blocking other notices; a manager that cannot act is skipped up the managed_by chain to the coordinator; UTC timestamps on every watch-agents log line; regression tests that fail before the fix; userguide note.
