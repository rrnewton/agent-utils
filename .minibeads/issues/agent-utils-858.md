---
title: 'chat-reply-wake-latency: chat reply --file waits 60-80 s for the next pass instead of waking the service'
status: closed
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T00:25:31.194759916+00:00
updated_at: 2026-11-05T04:30:00.000000000+00:00
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

## Resolution

[opus 5.5] cl-start, 2026-11-04. Not a regression in 0a2f112f..d6f0a4cf: the owner-loop code is unchanged there. The wake did arrive, but the owner loop takes reply wakes only at the top of a pass, and the slow replies (15-65 s) coincided with passes that spent 26-92 s typing into a busy pane. A reply sender thread now takes each `reply:<key>` wake from the listener and sends the stored replies through its own outbound transport, under a publish lock the owner loop's publish loop also takes (it never holds that lock while typing), so ordering and the thread post-rate breaker are unchanged. Any owner-loop step that takes 10 s or more is logged by name. Tests: a reply stored while a pass is held inside delivery typing reaches the fake helper in under 1 s, and a failing helper on the sender thread still trips the breaker after 8 charged sends.
