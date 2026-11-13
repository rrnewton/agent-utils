---
title: 'agent-teams-relay-to-queue-delay: accepted relays reach the peer queue minutes later'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-13T13:58:22.632040641+00:00
updated_at: 2026-11-13T14:59:03.842594942+00:00
---

# Description

Split from #1479. Cross-host agent-teams sends returned 'accepted' but reached the peer harness queue 7-11 minutes later (five timed messages), then surfaced one per turn over ~30 min (that second delay is #1479, fixed). Where the 7-11 min goes is not yet proven: candidates are chat-bridge ingestion (relay posts other bots make never arrive on the event stream, so they wait for the general rescan at 90s plus its 15s grace and settle, or for relay_poll_seconds when configured; #769 measured 77-540s against 2-3s direct) and rate-limit backoff in the bridge. The serve/accept/enqueue hop itself is synchronous and starts a bounded auto-drain, so it should not hold minutes.

# Design

Instrument first: stamp bridge receipt, relay-in, accept and queue-file times for one relayed message end to end and identify the holding stage. Then fix that stage (likely: enable or shorten the relay poll for configured relay channels, or carry the message on the stream path) with a timing test that does not wait on wall clock. Do not widen #1479's queue work.

# Acceptance Criteria

One relayed message's stage timings are recorded in the issue with the holding stage named. The fix (or a further split with evidence) lands with a non-wall-clock test. Senders keep the same accepted contract.

# Notes

Split from #1479 with the timing evidence recorded there (five message IDs, 7-11 min accepted-to-queue). Relay ingestion bounds in tree: stream 2-3s, relay poll 5-60s when configured, general rescan 90s + 15s grace (#769 relay-rescan-settle-latency).
