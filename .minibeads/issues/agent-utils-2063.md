---
title: 'vibe-talk-quote-reply-view-stability: quote-replying in two-column thread view must keep the quoted post still'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T12:26:00-05:00
updated_at: 2026-11-24T05:00:00-05:00
---

# Description

The owner, reading a thread in two-column mode and replying to a long message: "when i quote reply ... some junk updates lower on the screen and the post I'm replying to DOES NOT STAY STILL on the screen. This violates our view stability principle I believe".

# Design

Find what moves the quoted post when quote-reply starts (composer opening, quote insert, count or chip refresh, windowed re-slice) and anchor every such update to that post, so its top offset does not change.

[watermelon-full-preview] Findings (mu-quotestill, 2026-11-21): reproduced on a local
fixture of the same shape (a synthetic long message in a thread, served by the real
page and routes; the #2060 local clone cannot serve a thread-view timeline on any
current build, so it could not render this jump). Capturing the quoted post's
`getBoundingClientRect().top` every animation frame from the Quote-reply press
until the composer settles (95+ frames, then through Cancel):

- desktop 1280x800, post mid-column: -12.0 px in the first painted frame;
  low on screen: -229.0 px; phone 412x915: -27.0 px; windowed on or off and
  4x CPU throttling made no difference, and no later frame moved again.
- The one culprit: the show-the-box scroll at the end of `dockReply`
  (`vibe-talk/web/src/view/channel/replysession.ts`) -- `over = mount.bottom -
  pane.foot(); if (over > 0) nudge(scroller, over)`. In every case the
  scroller's `scrollTop` increase equalled the post's top decrease to 0.1 px,
  in the same frame the box appeared. No windowed re-slice, count or chip
  refresh moved it. A variant build with only that scroll removed measured
  0.0 px in every desktop case.

Fix: opening the reply never scrolls the list to show the box. Where the box
drawn under its row would sit below the column's foot, it is docked to that
foot instead -- painted there by a shift of the box itself over the rows,
whose places never move -- until the box under its row would stand inside
the foot (the reader scrolling it there hands it back). The older "move up
just as far as shows the box" outcome (`#247`, the low and tall cases of the
reply walks) is retired by the owner's view-stability rule; the reply and
inline-reply walks now assert stillness plus a visible box, docked at the
foot where it would overflow.

# Acceptance Criteria

- In two-column mode with a thread open, starting a quote-reply on a long message leaves the quoted post's top offset unchanged (0 px), at phone and desktop widths, with windowed lists on and off.
- Covered by a deterministic, frame- and condition-based browser check (no wall-clock budgets).

[watermelon-full-preview] Met by `vibe-talk/tests/quote_still_browser.py`
(dag step `quote-still-browser`): eight cases (1280x800 two-column thread and
412x915 one-column thread, windowed on and off, the post mid-column and low
on screen) assert 0 px in every captured frame through opening and Cancel,
and that low on screen the box stands visible docked at the column's foot.
Measured on the fixed tree: worst 0.0 px in all eight cases.

# Notes

## Resolution

Closed 2026-11-24 after verification on the deployed tree (2dd8642a, the
build coordb deployed): `vibe-talk.check.quote-still-browser` passes there
in 32s, worst 0.0px across all eight frame-captured cases (two-column thread
at 1280x800 and one-column thread at 412x915, windowed on and off, post
mid-column and low on screen). The screenshot matrix (light and dark, phone
and desktop, two-column thread view with the quote-reply open, box docked
visible at the foot) was captured from the same tree and inspected: the
quoted post stands highlighted and still, the reply box under it. The one
neighbouring item the handoff held for the owner's answer (cap the dock
lift at the quoted post's last line, phone low case) is separate follow-up
work on lane mu-dockcap (#2119), not part of this issue's acceptance.

Related earlier anchoring work: #1983, #2008, #2009. Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

[watermelon-full-preview] One neighbouring case left as it was, named rather
than silently absorbed: a plain Reply on a Main root that gathers its thread
under it can still be carried by the foot-following keeper (`#280` family)
where the page believes the reader is following the foot. That keeper's
contract is the reader-at-the-foot's, not the quote-reply dock path's, and
the inline-reply walk's harness now arrives by the wheel as a reader does
before it judges that case.
