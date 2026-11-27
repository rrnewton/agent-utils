---
title: 'vibe-talk-read-strip-foot-same-frame: on a phone the list''s foot reaches the read strip''s top a frame late, so the list runs under the strip'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- read-aloud
created_at: 2026-11-26T22:25:00-05:00
updated_at: 2026-11-26T22:25:00-05:00
---

# Description

On a phone, when a read starts and the read strip appears, the list's foot is pulled up to the strip's top asynchronously: for a moment the list runs exactly the strip's height (93.0 px) under the strip. read-aloud-blocks-browser's one-shot phone layout check (`check_phone_layout`, right after the read reaches "1 of 7") catches that moment about 40% of the time at load1 121-162, on main (cl-flaketriage's runs). The liaison chose the product fix over loosening the check: the list's foot moves in the same frame the strip appears (and goes in the same frame it leaves), so there is never a transient overlap, and the one-shot check stays as the guard.

# Design

- The list's foot follows the strip in the frame it appears and the frame it leaves: the strip's height is known to the list's frame when the strip is shown, not one measurement later (today `#screen-main` keeps `--read-steer-height`, written from a measure of the strip after it is up).
- The scroll owner holds the row being read across the change (R12, owner rule 370): it keeps its top within 0.5 px per frame as the strip appears and disappears.
- The one-shot `check_phone_layout` stays as written; it becomes the guard.

# Acceptance Criteria

- read-aloud-blocks-browser's check passes 10 runs out of 10 alone on the fix (load1 recorded per run), against the same check red on main.
- A per-frame trace (the list's foot against the strip's top, and the reading row's top) from the read's start through the strip appearing and from the stop through it going, before (main) and after, with a JSON sidecar: no frame with the list under the strip, reading row <= 0.5 px.
- Shots of the playing state (phone light and dark, keyboard closed; desk), each folder with its BUILD.txt, and the advisory design review against vibe-talk/docs/ui-rules.md.

# Notes

Owner-UI fast-track (liaison 995 chose fix B, the product fix). Related: #2077 vibe-talk-browser-checks-hot-host (read-aloud-blocks-browser is on its CPU-timeout known-red list), #998 gate-browser-steps-under-load. Logs of the red runs on main: cl-flaketriage's readaloud runs (2026-11-26).
