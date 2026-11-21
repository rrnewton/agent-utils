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
