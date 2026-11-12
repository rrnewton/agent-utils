---
title: 'discuss-cache-only: Discuss reads only the message cache, never upstream'
status: open
priority: 0
issue_type: bug
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T11:05:23+00:00
updated_at: 2026-11-12T11:05:23+00:00
---

# Description

[watermelon-full-preview] Owner reply 414 (hard rule): "this summary agent should have NOTHING to do with upstream slack. It should only work on existing info in the vibe-talk cache. We should verify the queries into the cache are VERY FAST and that the only time is waiting on the model."

Live evidence 2026-11-11: `POST /api/v1/channels/C0CC755QYSK/catchup` returned 502 in 1-11 ms (`Slack RATE LIMIT ... on conversations.replies / conversations.history ... waited 0.0s`) after repeated presses drained the provider buckets. Slice 1's `ops::catchup_fetch` walked `ops::timeline` upstream (history plus one `conversations.replies` per thread root; cache-first excluded server reads), so a catch-up could wait on — and spend — upstream rate limits, against the owner's speed rule (reply 376).

 Parent issue: #1316 catchup-side-agent.

# Design

- Channel and thread windows read ONLY this server's message cache: the newest held page (`Sink::stale_timeline`), then steps back from the file (`Sink::held_below`), inside the existing recap bounds. No provider call is made, and none is begun behind the answer (no cache-first refresh).
- If the cache holds nothing of the view, the answer says so (thread: open it once so the server holds it) with no model call; if it holds part, the window is what is held and `truncated` says so. The model is never sent to fetch.
- The latency stamp splits cache ms / model ms / output chars (panel debug line and server log), so cache time is visible separately from model wait.
- The read set, noise rules, labelling, footer ids, and the `discuss` read-reason tagging are unchanged; a test pins provider call count at zero across channel, thread, and cache-miss reads.

# Acceptance Criteria

- `catchup_fetch` (and therefore the endpoint) makes zero provider calls in channel (flat/main), thread, and cache-miss cases, asserted by a counting chat fake whose count does not move.
- A cache-held channel answers from the cache with the same window, labels, unread set, and footer ids as slice 1; a cache-miss thread answers the holds-nothing sentence with no model call.
- The stamp/panel line reads cache ms / model ms / output size; existing Discuss tests and walks (390x844, 393x852, 1280x800) pass; gated at owner tier and landed on its own.

# Notes

Owner replies 414 (cache-only hard rule), 376 (speed), 413/415 (slice-2 panel programme). Number reserved by coordb. Filed by mu-catchup; implementation lands separately from the other slice-2 issues.
