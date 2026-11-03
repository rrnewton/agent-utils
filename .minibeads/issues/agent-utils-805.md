---
title: 'relay-poll-receipts-during-hold: read receipts on our in-custody relay threads while the Slack budget holds'
status: open
priority: 2
issue_type: bug
labels:
- chat
- agent-teams
created_at: 2026-11-03T02:02:46.739260689+00:00
updated_at: 2026-11-03T02:02:46.739260689+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator from the #769 relay-rescan-settle-latency "after" probe (one host to another, 6 direct and 6 relay messages). Relay one-way delivery is now a median of 26 s (p90 about 50 s; it was 91-540 s before #769), against 2.4 s for direct. But the relay ROUND TRIP, until the sending bridge reads the receiver's receipt, is a median of 174 s (p90 about 515 s). The receiver posted its receipts promptly (median 27 s). The delay is on the reading side: after a Slack rate limit, the shared per-host call budget holds, and the relay poll lists 0 threads ("next poll in 30 s", 37 times in one hour). The receipts are thread replies, so they sit unread. Slack rate-limited the host's listing 4 times in that hour; the owner identity is shared by both bridges on the host and by vibe-talk.

# Design

1. During a budget hold, the relay poll still lists the threads of this bridge's own in-custody relay posts (posts awaiting a receipt), bounded to a small number of calls per poll, so receipts are read even while general listing waits.
2. Before raising any poll rate, find what triggers the rate limits: a per-method limit on conversations.replies, the other bridge or vibe-talk listing as the same user, or the burst shape of the relay poll itself. Record the finding in the issue.
Context: the teams stay in mirror mode (relay one-way is about 10x direct), so this is about receipts and alarms, not about making relay the primary route.

# Acceptance Criteria

- With a budget hold active, a receipt posted to one of this bridge's in-custody relay threads is read within one poll interval; a test covers it with a fake rate limit.
- The calls made during a hold stay within the stated bound.
- The issue records which caller or method triggers the rate limits, with evidence from the plugin logs.
