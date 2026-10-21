---
title: 'threads-unpack-all: a toggle in the Threads view unpacks every thread''s messages under its card'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-21T15:45:24.359756603+00:00
updated_at: 2026-10-21T19:08:28.579094515+00:00
closed_at: 2026-10-21T19:08:28.579094394+00:00
---

# Description

[muse watermelon] The owner, 2026-10-21: in All he can bridge one thread at a time with its N Replies button (the children come up to meet the parent, which stays put), but bridging another thread breaks the first. He wants every thread chained at once in one stream. Design (ai_docs/transient/2026-10-21-threads-unpack.md): in the Threads view (Cmd+3), which already lists threads by last updated, a per-device toggle (Unpack all chip, key u, listed in the ? help) draws each thread's messages under its card in the same bridged style as #215 reply-coalesce, tinted by the thread colour. Show/Collapse/Hide read applies inside each block; collapsed read runs are the grey N read messages line joined to the spine by a short stub. Blocks lazy-load their thread as they scroll into view (at most 3 reads in flight) so a channel with many threads stays responsive; render cost is measured on a 40-thread channel.

# Acceptance Criteria

Toggle off is unchanged. Toggle on in the Threads view: every card gains its messages beneath it in time order with a continuous bridge in the thread colour; card order unchanged; two-column cards unchanged. Under Collapse read a run of 2+ read rows in a block is one grey line with a connector stub to the spine, tappable to open; under Hide read read replies are absent from every block. State persists across reload; u is in the ? help and as aria-keyshortcuts on the chip. Page tests fail before the change with negative controls; Chromium screenshots at 1280x800 and 393x852, dark and light, on and off, with collapsed runs.
