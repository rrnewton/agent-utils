---
title: 'vibe-talk-page-freeze: the phone page locks up and stops answering touch; record freezes and find the cause'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T12:44:00-05:00
updated_at: 2026-11-21T12:44:00-05:00
---

# Description

The owner: "The Android app is actually locking up for me a fair bit and after it doesn't respond to swipes or presses of my finger for a while, I just restart it. It's locked up right now."

The server stayed healthy and fast through the reported lockup, so the freeze looks page-side (main thread blocked in the WebView). Around it the server log shows slow requests on the owner's path: a speech prepare that fetched upstream for about 28 s, older-page reads that each waited the 1.5 s provider stand-in, and a client-settings read of about 2 s that should be instant.

# Design

1. Freeze telemetry: a main-thread watchdog in the page that records long tasks and missed-heartbeat freezes with attribution (long-animation-frame scripts where available, current action, view and layout mode), keeps them locally, and posts them to the server on recovery or next load; counted per day.
2. Find and fix what blocks the main thread, starting from the scroll traces and the telemetry.
3. Explain the slow server reads (client-settings, older-page stand-in waits, the upstream speech prepare) and fix any that sit on an interactive path.

# Acceptance Criteria

- Freezes and long tasks on the phone are recorded with attribution and reach the server after recovery or reload.
- The identified blocking work no longer blocks input, shown by a deterministic browser check (frame- and condition-based, no wall-clock budgets).
- client-settings never waits on other work.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

[watermelon-full-preview] 2026-11-21, owning lane (mu-freeze). Owner addendum (same day): left alone, the app recovers by itself after an app switch, laggy at first — so the watchdog also records every hide/show with timestamps, the backlog measured at the show (live replays waiting, inputs heard while hidden, zero-timer and first-frame lateness), and how soon after the show the first stalls came. Presses during a pending speech prepare coalesce latest-only through the reader's ticket (`#2058 vibe-talk-reader-arrows-instant`, a sibling lane; not forked here), so a show never fires a queue of presses.

Findings so far, each proven in code: (a) client-settings waits because the whole state store is one SQLite connection behind one mutex taken on the blocking pool — the trivial read queues behind any long holder; (b) an older-page step back whose cache claims cannot prove the stretch whole races the provider for 1.5 s and then answers an EMPTY page (more above, same cursor) under the same `cache-older` log label as a proven instant cache answer; (c) speech prepare already checks recent memory and the message cache first, but one miss makes the whole batch await an unbounded upstream fetch, and a tap can be waiting on it; (d) the speech GET durations in the log are time-to-first-audio (the access clock stops when the headers go out), and the reply-outcome duration is its long poll reporting a slow send — neither is a server stall of its own. Page-side suspects for the blocking work itself: a reply-landing animation-frame loop that rewrites a clamped scroll position every frame, unguarded same-value scroll writes on held redraws, and a synchronous whole-list drain inside the coming-back-into-sight task; the telemetry lands first so the phone itself can say which.

Landing order (coordinator-approved, each its own commit and gate): A telemetry, then C1 client-settings snapshot, C3 bounded speech prepare, C2 partial older pages. Part A: always-on page watch (`freezewatch.ts`: heartbeat gaps in sight, long tasks and long animation frames with the frame's invoker, hides/shows with resume backlog; attribution = view/screen/channel view, one or two columns, read-aloud state with `pending` for a press awaiting prepare, windowed, last act by kind), a bounded on-device ring posted on recovery/show/next load, and a closed `POST /api/v1/freeze-report` (own log budget, per-day totals in every header line, no ids and no screen words). No visible UI change.
