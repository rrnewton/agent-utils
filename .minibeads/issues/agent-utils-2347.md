---
title: 'vibe-talk-read-strip-foot-same-frame: on a phone the list''s foot reaches the read strip''s top a frame late, so the list runs under the strip'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- read-aloud
created_at: 2026-11-26T22:25:00-05:00
updated_at: 2026-11-27T00:10:00-05:00
closed_at: 2026-11-27T00:10:00-05:00
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

Cause (2026-11-26, a copy of the check's 412x915 phone walk with a MutationObserver on `data-read-strip` and per-frame and post-frame samples): the read's tap shows the strip and sets `data-read-strip` in one task, and `#screen-main` keeps the strip's room as `padding-bottom: var(--read-steer-height)`, a value written only by the strip's ResizeObserver at the next rendering update. Between the two, every layout read saw the list's foot 93.0 px under the strip's top (padding 0px), and so did the next frame's requestAnimationFrame; that frame's observer then wrote 93px. `wait_position("1 of 7")` often resolves in that window. The reading row did not move (83 px in every sample); taking the strip away was already same-task.

Fix: the observer's body is one function (`measureReadSteer` in web/src/app/main.js), which the read's layout (`renderReadLayout`) runs at the end of the task in which it showed, hid or changed the strip (a microtask, once the same pass has settled the strip's own rows, so the height is the final one); the observer stays for later size changes. Main 3 reds in 11 runs ("the list runs 93.0px under the strip" at 393x852 and 375x812), the fix 10 of 10 green and no frame with the list under the strip.

## Resolution

[opus 5.5] Landed in eb2cacde (2026-11-27 12:08 AM ET, liaison LAND NOW, owner gate fully green on the gated head 75a872b7). The read strip's measure (`measureReadSteer`) now runs at the end of the task that shows, hides or changes the strip, so the list's foot moves in that task, not at the next rendering's observer. The one-shot `check_phone_layout` is unchanged and is the guard.

Evidence: read-aloud-blocks-browser alone, 10 of 10 green on the final tree and 3 of 3 on the gated head (load1 104-146), against 3 reds in 11 runs on main ("the list runs 93.0px under the strip" at 393x852 and 375x812). Per-frame trace of the 412x915 walk: main showed the list's foot 93.0 px under the strip at the frame after it appeared; the fix 0.0 px at every frame, appearing and leaving; the reading row did not move in either build. Shots of the playing, paused and stopped strip at 393 and 412 (light and dark) taken on the gated tree, byte-identical to the reviewed set; advisory design review found nothing new against main. A foot-pinned reader's message moving 47 px as the strip comes and goes is #2351.

