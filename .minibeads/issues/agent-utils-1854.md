---
title: 'vibe-talk-fold-pinned-target-inline: keep the pinned target beside the fold separator text'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- ui
- pins
created_at: 2026-11-18T12:15:00-05:00
updated_at: 2026-11-18T12:15:00-05:00
---

# Description

[watermelon-full-preview] Owner-UI follow-up to #1850 vibe-talk-fold-separator-shows-pinned (landed 1990702f). Liaison review nit: on desktop the separate "1 pinned" target sits at the far right edge of the separator, detached from its text "… 6 read messages, 1 pinned · Jan 5, Mon …". Keep the target right next to the text (inline after it, or making the "1 pinned" words in the text the target itself), on phone and desk, with the dashed rule lines still spanning both sides. Keep both targets keyboard and screen-reader accessible.

# Design

- The "1 pinned" words in the separator sentence become the pinned target itself: the main line keeps "… N read messages,", the pinned button carries "M pinned", and a trailing span carries the days and closing ellipsis, so the three pieces read as one sentence with no duplicated wording.
- The head row draws its dashed rules on both sides of the whole inline group (left rule, main line, pinned target, tail, right rule) only while a pinned target is shown; a run without pins keeps the existing full-width line unchanged.
- Both targets stay real buttons with their distinct aria-labels ("Show N read messages" / "Show M pinned messages in this folded run"); the trailing days span is aria-hidden, as the days were already absent from the main line's aria-label.
- The full sentence stays available as one string (`readRunText`) for change detection and unit wording; the head/main/pinned/tail pieces are derived from it in present/rows.

# Acceptance Criteria

- On desktop and phone, the pinned target is immediately beside the separator text (bounding-box gap under a small bound on desk), and the separator still reads "… N read messages, M pinned · <days> …" as one sentence.
- Dashed rule lines span both sides of the inline group; a fold without pins is unchanged.
- Both targets remain keyboard-focusable and screen-reader labelled; tapping the pinned target reveals only pinned rows, tapping the main line expands the whole fold, as in #1850.
- `vibe-talk/tests/fold_pins_browser.py` asserts the target is adjacent to the label text on desk (bounding-box gap under a small bound), in light and dark, phone and desk.
- Screenshot matrix (light/dark, keyboard open/closed, phone/desktop, Discuss half-pane on phone, before/after) captured on a test instance serving the real routes and inspected; dynamic end-to-end exercise against the real server router with the page's exact requests.

# Notes

Owner-priority gate, deployed on landing. Lane mu-foldpins2, coordinator coordb. Prior work: #1850 vibe-talk-fold-separator-shows-pinned, `vibe-talk/tests/fold_pins_browser.py`, commit 1990702f.
