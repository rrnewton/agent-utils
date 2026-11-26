---
title: 'vibe-talk-key-scroll-freezes-11-26: the page never blocks typing or scrolling for seconds on a busy phone channel'
status: open
priority: 0
issue_type: bug
assignee: cl-freeze
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T03:45:00-05:00
updated_at: 2026-11-26T03:45:00-05:00
---

# Description

Owner request 379 (2026-11-26 3:31 AM ET), verbatim: "Very laggy client right now. Can you see any of that server side? Can I have a debug mode with some local client logging or sampling that gets uploaded along w the scroll traces. My [busy project] channel updated very slowly. Was laggy while refreshing. Screen jumped a lot while I was trying to type a message. And the typing itself was laggy and got frozen some."

The page's freeze watch (`#2066 vibe-talk-page-freeze`) posted 178 freeze reports in 16 minutes around 3:15-3:31 AM ET, 34 of them at or above 1 s. The worst: a long task of 4330 ms and a freeze of 4728 ms with the last act a key press, and a long task of 4258 ms plus a freeze of 5729 ms with the last act a scroll, about 3:30 AM ET. All came from the phone layout (single column) in the flat All view of a busy channel, and the script attribution was empty in every report. The server side was clean in the same window: every request answered in at most about 100 ms, no errors.

So the main thread is blocked for seconds by page work that runs per keystroke, per scroll or per live arrival, and the reading row moves while the reader types.

# Design

1. Reproduce on a local test server built from the deployed revision, with a long busy channel in the flat All view at phone size: type in the composer and scroll while live arrivals and refreshes land, with long-frame attribution and a profiler on.
2. Find the work that holds the main thread for seconds, by file, function and trace. Candidates: the night's landings that run per keystroke or per arrival (the search head-room and foot-room passes, the Discuss split-pane redraws, the strike index, the older-walk event re-step) and any whole-list layout read on arrival.
3. Find what moves the screen while the reader types.
4. Report the root cause before any fix; then fix it so that per-keystroke and per-arrival work is bounded by what changed, not by the channel length.

# Acceptance Criteria

- The root cause is shown with a trace on a local server from the deployed revision (function-level attribution), recorded with the host load at the time.
- With a long busy channel at phone size, typing and scrolling while arrivals land never produce a long task or freeze above the freeze watch's threshold attributable to the found path; a page check counts the guilty work (calls, rows touched, layout reads) per keystroke and per arrival against a ceiling, never wall time.
- Typing does not move the reading row (anchor text and position unchanged around the key press, per-frame trace within 0.5 px).
- Holds with the host at or above the hot-load line.

# Notes

Owner-UI fast-track. Owning lane: cl-freeze. Related: `#2276 vibe-talk-client-debug-mode` (the attribution instrument, built in parallel), `#2066 vibe-talk-page-freeze` (the freeze watch), `#1976 vibe-talk-client-perf` (standing perf lane), `#2278` (server side of the same report).
