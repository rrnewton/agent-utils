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
updated_at: 2026-11-26T06:00:00-05:00
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

## Resolution

Root cause, traced on a local server running the deployed build against a read-only copy of the channel's message cache, at phone size in the flat All view with the CPU throttled 4x (load1 74-145, not hot):

1. Freezes: the page wrote its measured geometry as custom properties on elements every row inherits from: the box's top edge (`--composer-top`, view/shell/chips.ts `placeChipsOverComposer`) on `#frame-body`, the chips' and the read-aloud strip's room and height on `#frame-body` and `#screen-main` (app/main.js, two ResizeObservers), and the keyboard's room on the root (view/composer/keyboard.ts). Custom properties inherit, so every change restyled the whole document. The box's top edge changes on every scroll at the foot. The scroll listener (view/channel/listwatch.ts) wrote it and then read the foot (`renderJumpNewest` → `footGap`), which forced that restyle at once. Trace: each of these restyles covered all 12.5k elements at 192 rows, 423-583 ms at 4x; layout itself touched 13 objects. The cost is linear in rows: one write plus one read took 314/520/994 ms at 4x for 192/384/768 rows.
2. Screen jumps while typing: two keepers disagreed about where the composer's foot is. The keyboard's jump (`fitComposerToKeyboard`, `#610`/`#1135`) put the box's foot on the keys and parked the Send row under them; the keeper that holds the composer while rows change (`keepComposerInView`, `#1839`) brought the whole composer, row included, into view. Each row change while typing raised the list by about 70 px and the next jump dropped it back. A third keeper (the foot keeper's `footTop`) went 8 px past the jump's foot and was brought back too.

Fix:

- The geometry properties are registered `inherits: false` and written only on the elements whose own rules read them (web/src/view/dom/geometry.ts `GEOMETRY_CONSUMERS`); the chips' foot is worked out on each element that stands on it; the per-row search-hit scroll margin moved to the scroller's scroll padding while a search is open. A browser without `@property` (Chrome/Android WebView before 85, Safari before 16.4, Firefox before 128) inherits them as before: correct, just as slow.
- The scroll pass measures the chips before the jump reads the foot and writes after it.
- One composer foot: the keyboard's jump brings the whole composer, Send row included, to the keyboard's top, and `keepComposerInView` keeps the same foot (`fittedFoot`). The room past the fitted foot counts as room, so the foot keeper stops where the jump does. With the row's own Send in sight, the corner Send stands down, which covers `#2274` for the fitted channel composer.

Before and after on the same clone, 768 rows (drawn rows cloned for length), 4x: a scroll pass at the foot restyled 49,961 elements before and 16 after. Typing 40 keys at the foot with the keyboard up while row changes land: 28 opposite-direction scroll writes before, 0 after; longest long animation frame 4500 ms before, 654 ms after; longest long task 2753 ms before, 490 ms after.

Gating check (counts, never time): tests/geometry_restyle_browser.py (phone and desktop, 260 rows). The page before the fix fails all three parts.

Follow-ups: `#2283 vibe-talk-root-has-rules` (the root-anchored `:has()` rules, the next page lane), and windowed lists on by default (evidence pending).
