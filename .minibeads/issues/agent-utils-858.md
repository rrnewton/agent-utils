---
title: 'chat-reply-wake-latency: chat reply --file waits 60-80 s for the next pass instead of waking the service'
status: open
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T00:25:31.194759916+00:00
updated_at: 2026-11-05T00:25:31.194759916+00:00
---

# Description

On one host's bridge, since its refresh to d6f0a4cf (2026-11-04 14:08 ET), `chat reply --file` replies take about 60-80 s from "stored" to the outbound helper starting the send. Before, it was 1-3 s. Example: a reply stored at ~18:10:40 ET; the helper started the send at 18:11:58 and posted it at once. Health is OK and the reply breaker is clear. Hypothesis: `chat reply` no longer wakes the running service (`"service_woken": true` is still printed), so the send waits for the next periodic pass.

# Design

- Bisect between 0a2f112f (fast) and d6f0a4cf (slow). Candidates: #831, #832 (trace metadata and the helper change), #834 and #840.
- Fix the wake path so a stored reply reaches the helper within a second.

# Acceptance Criteria

- A test with a fake helper measures reply-stored -> helper-started at under 1 s with the service running.
- Gate passes.

# Notes

P1 via the lead (2026-11-04). Assigned to cl-start after #852 P0a, or alongside it if it is the same area.
