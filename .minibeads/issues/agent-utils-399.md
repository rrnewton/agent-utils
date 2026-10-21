---
title: 'slack-quiet-cursor: a quiet channel lets the Slack cursor fall out of the event stream''s retention'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- slack
created_at: 2026-10-21T01:12:49.900795184+00:00
updated_at: 2026-10-21T01:12:49.900795184+00:00
---

# Description

[opus 5.5] On 2026-10-21 01:00Z the Slack bridge was restarted about 11 hours after the last message in its channel. The plugin resumed with --catchup=223201 and the client answered with a retention overflow; the plugin reported a Gap and the host (correctly, protocol v1) refused to reconnect until an operator checked the channel and started a fresh state.

Cause: the plugin's cursor advances only when the tail prints an event for the configured channel. The tail filters client-side, so the stream's sequence (one queue per subscribing user, every source) keeps moving while the plugin sees nothing; its retention is about 8 hours. In the incident the head moved from 223201 to 225029 while the channel was silent. Any restart after a quiet spell longer than the retention hits a gap.

HEARTBEAT lines carry no sequence, and heartbeat refreshes print none. The one position the client exposes is its startup subscribe response, after catch-up has drained to the queue head (filtered events included).

# Design

Commit the startup subscribe position as a checkpoint only when it is proven: the tail was started with --catchup=<S> in explicit mode, its standard error showed 'Catching up from S' then 'Fetched N events' (printed only when catch-up ends normally), and the first SubscribeResponse has seqId == requestedPositionSeqId == C >= S and lastIssuedSeqId >= C. Catch-up events are flushed to standard output before that line is written, but the plugin reads the two pipes separately, so it also waits for a HEARTBEAT on standard output: every heartbeat is written after the subscribe response, so every catch-up event at or before C has been read by then. To keep a long quiet session fresh, the plugin reopens the tail from its committed sequence when the cursor has not moved for cursor_refresh_seconds (default 7200, well inside the retention).

# Acceptance Criteria

Fake-transport tests: a quiet channel plus heartbeats advances the cursor; an event racing the heartbeat or the subscribe line is never skipped; a restart after the advance resumes without a gap; unproven positions (no Fetched line, override mode, seqId != requested, time cursor) commit nothing. make validate passes.
