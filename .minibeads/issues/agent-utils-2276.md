---
title: 'vibe-talk-client-debug-mode: an owner switch that records client freeze attribution and uploads it with the scroll traces'
status: open
priority: 0
issue_type: feature
assignee: cl-debugmode
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T03:40:00-05:00
updated_at: 2026-11-26T03:40:00-05:00
---

# Description

Owner request 379 (2026-11-26 3:31 AM ET), verbatim: "Very laggy client right now. Can you see any of that server side? Can I have a debug mode with some local client logging or sampling that gets uploaded along w the scroll traces. My [busy project] channel updated very slowly. Was laggy while refreshing. Screen jumped a lot while I was trying to type a message. And the typing itself was laggy and got frozen some."

The page already posts freeze reports (`#2066 vibe-talk-page-freeze`: kind longtask/freeze, the reader's last act, view, layout) and scroll traces. In the night's 178 freeze reports the script field was empty in every one, so nothing says what held the main thread. This issue adds the instrument; the standing perf lane finds and fixes the cause of the night's freezes separately (`#2277`).

# Design

A client debug mode: one owner switch, off by default, kept as a device setting and also reachable from the settings screen. When it is on, the page records and uploads, along with the existing freeze reports and scroll traces:

- Long Animation Frames attribution (`PerformanceObserver` type `long-animation-frame`: each script's source URL, function name, invoker and duration, plus the frame's render start and style-and-layout start) where the browser supports it, falling back to `longtask`.
- Named spans (`performance.mark`/`measure`) around the page's own hot paths: key and input handlers, the search passes, arrival and redraw, scroll-owner writes. Cheap enough to leave on while the switch is on.
- Main-thread sampling with the JS Self-Profiling API where available (the server sends the `Document-Policy: js-profiling` header only while debug mode is on), summarised to the top frames per freeze.

Rules: timings and code names only, never message text, ids or tokens; bounded size per upload, rate limited, no upload storm; the scroll-trace route's existing caps apply. With the switch off nothing extra is registered and nothing extra runs. The server takes the new fields on the existing freeze-report and scroll-trace routes and logs them (timings and code names only), so the attribution can be read from the server log once the owner turns the mode on.

# Acceptance Criteria

- Switch off: no extra observers registered, no extra marks, no profiler, no extra upload fields (page test).
- Switch on: long-frame attribution, named spans and (where available) profiler top frames are recorded, capped, and uploaded with the freeze reports and scroll traces (page tests at phone and desktop sizes).
- A real-server test: the routes accept the new fields, reject oversize or malformed ones, and log them; contract checks updated.
- Exercised on a local test instance through the real server, with the screenshot matrix for the settings switch.

# Notes

Owner-UI fast-track. Owning lane: cl-debugmode. Related: `#2066 vibe-talk-page-freeze` (the freeze watch this extends), `#2091 vibe-talk-shrink-anchor-policy` (its live invariant debug mode uses the same freeze channel), `#2277` (the night's freeze root cause, standing perf lane).
