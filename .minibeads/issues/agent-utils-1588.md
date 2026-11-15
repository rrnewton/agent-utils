---
title: 'autoplay-follow-scroll: smooth-scroll so the highlighted part stays in view during autoplay read-aloud'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T16:13:00.000000000+00:00
updated_at: 2026-11-15T16:13:00.000000000+00:00
---

# Description

Owner UI request 097. Autoplay (read-aloud) proceeds to the next unread
message when a read finishes, but the screen did not scroll to follow the
highlighted bit. The screen now smooth-scrolls so the highlighted sentence —
or, with no sentence light, the row being read — stays in the middle third
of the visible list, both when autoplay moves to the next message and as
the highlight advances within a long message.

# Design

`vibe-talk/web/src/view/voice/followscroll.ts` holds the pure rule (middle-
third band, centre-on-exit delta, glide choice, pause state);
`read-aloud.ts` supplies live rects and scrolls `#scroll-area` by the delta
with `behavior: "smooth"` (`"auto"` where the stylesheet says a
reduced-motion reader glides auto, `--column-glide` on `#screen-main`, the
word the column glide already reads). Hooks: read start (where autoplay's
next message begins), `showReadingSentence` (sentence advance) and
`showReadingPosition` (block boundary). The port is `#scroll-area` as laid
out — with Discuss docked it ends at the panel's top edge (#1526), so no
panel-specific math is needed. Pause rule: a scroll the follow did not
cause pauses following until the highlight is back in the band or another
read starts; the follow's own scrolls never
pause it, and nothing scrolls on a timer, so a reader dragging or flicking
is never fought. No `voice.js` / `src/app/main.js` edit: the highlight and
read-start signals already live in `read-aloud.ts`.

# Notes

Tests: `tests/js/view/voice.test.mjs` gains the band/delta/behaviour/pause
cases. Browser: `tests/autoscroll_follow_browser.py` serves the page with a
fake local-voice API at 390x844 and desktop sizes, turns Autoplay on, and
asserts by condition wait that a later message is read with its row in the
middle third and the list scrolled. Screenshots from that walk are
inspected before landing.

[opus 5] Implemented on branch `mu-autoscroll/1587` from `github/main`.
Renumbered to 1588 after the owner's own filing took 1587 on main.
