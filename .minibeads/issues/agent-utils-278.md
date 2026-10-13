---
title: 'timeline-single-flight: one upstream read for identical /timeline requests, and every read tagged with its reason'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-13T20:02:21.921336242+00:00
updated_at: 2026-10-13T20:02:21.921336242+00:00
---

# Description

Remote-ops audit (2026-10-13) items W4 and W0.3.

W4: ops::timeline has no single-flight and no memo, so N identical /timeline requests make N upstream reads, and a page request abandoned at 20 s cancels the server handler while the provider read runs on; the re-send then starts a second read (111 abandoned and 46 failed fetch_timeline in one profiled hour). Make identical requests (channel, view, thread, before, decoded after, limit) share one read, run as a detached task so an abandoned request does not cancel it and its re-send joins it, with a short memo for newest pages (configurable, 0 turns it off). Allowlist before the join; stamping, noise, naming and read state stay per caller. Route ops::messages through read_ahead::window_turn, so concurrent window reads of one channel share one fetch too.

W0.3: tag every /timeline and recent read with why it was made. The page sends reason= on its timeline requests; server-internal reads carry their own (naming, recent, probe, summary-window, speech, ...). Log the reason in the access line and count upstream reads per reason in upstream_profile, so the next load figure is attributable.

Proof: tests count upstream calls per user-visible action with testing::CountingChat (5 concurrent identical requests -> 1 read; dropped at 100 ms and re-sent at 200 ms -> 1; a different view -> 2; a newest read 4 s later -> 2; a read-token join never queues naming), and tests/logging.rs asserts reason="poll" in the access line and other for an unknown value.
