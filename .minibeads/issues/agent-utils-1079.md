---
title: 'documents-chronological: Documents list newest at the bottom with drag-up refresh'
status: open
priority: 1
issue_type: task
created_at: 2026-11-08T17:56:46.831874072+00:00
updated_at: 2026-11-08T17:56:46.831874072+00:00
---

# Description

Owner reply 232 (2026-11-08). The Documents list today shows newest at
the top in reverse chronological order, while the message views are
forward chronological with newest at the bottom, opening scrolled to
the bottom. The owner asks to flip Documents to match the message
views, so the same drag-up gesture at the bottom refreshes both, with
a small refresh animation only if it comes from the platform or is
otherwise cheap and near-native. No hand-built physics is wanted.

Today the list sorts newest first under a "Newest first" heading,
pages older documents when scrolling near the bottom, and refreshes
on a pull down at the top. The channel view already supports both a
pull down at the top and a pull up past the newest message, with its
own indicator. Scroll paging of older documents, jumping to a
mentioning message and back, and the periodic live refresh must all
keep working after the flip.

# Design

The list sorts forward chronological, oldest at the top and newest at
the bottom, and its heading says so. The server contract does not
change: the first page is still the newest window and older pages are
still fetched with the same cursor. Paging inverts direction: older
documents load when scrolling near the top and land above the loaded
window with scroll anchoring that keeps the card under the reader's
eye in place. Entering the Documents list at its newest end opens
scrolled to the bottom, like the message views; returning to a
remembered place, including Back from a mentioning message, restores
that place instead. A refresh keeps the reader's place unless they
were at the bottom, in which case it follows newly appended documents
to the bottom; this applies to the gesture refresh and the periodic
live refresh alike. Arrow-key movement follows the new spatial order,
down toward newer documents.

The refresh gesture flips with the list: a drag up starting parked at
the bottom refreshes the Documents list, using the list's own touch
handling at the bottom edge. The channel gesture is not shared as
code, because its owner is wired to channel reads, channel freshness
and channel scroll suspension; the Documents gesture mirrors its
meaning (parked at the newest end, drag past the edge, release to
refresh) without generalising that owner. A drag at the top no longer
refreshes; the top is where older documents page in.

For animation, both lists scroll in the same scroller, which already
keeps overscroll contained while leaving the platform edge effect
visible, and the document itself allows no overscroll. On Android
Chrome, including the installed standalone app, that leaves the
native stretch at the edge with no new code. On iOS the app runs in
Safari by design and the inner scroller's native bounce is whatever
the platform gives; this change adds no setting that would alter it.
No custom spring, transition physics or new indicator animation is
built; Documents keeps its status-line confirmation. If a physical
device later shows no edge effect, that is reported rather than
papered over with hand-built motion.

Planned files: the Documents list module for order, paging, anchoring
and the gesture, the Documents view composition for entry and refresh
scroll handling, and the module and browser tests for the list. The
bottom-bar work in the companion issue lands first if the two touch
the same lines.

# Acceptance Criteria

- The Documents list shows oldest at the top and newest at the
  bottom, its heading says so, and opening it at its newest end lands
  scrolled to the bottom.
- Scrolling near the top pages older documents in above the window
  without the reading position jumping; the end of history stops
  paging cleanly.
- Jumping to a mentioning message and coming Back returns to the
  Documents list at the place it was left, and the periodic live
  refresh neither drops the reader mid-list nor strands a reader at
  the bottom above newly arrived documents.
- A drag up parked at the bottom refreshes the Documents list and
  confirms on the status line; the channel view's pull gestures are
  unchanged; no drag at the Documents top refreshes.
- No hand-built refresh physics is added; the platform edge effect on
  the shared scroller is the animation on both views, with any
  platform gap stated in the completion note.
- Focused module tests cover order, paging direction and anchoring,
  entry position and the gesture, and browser checks with screenshots
  are attached to this issue.

# Notes

Owner reply 232, 2026-11-08 12:29 PM ET. Companion to the bottom-bar
issue from reply 231, as a separate commit under one validation gate;
reply 231 goes first where they collide. Native availability, from the
shipped configuration: Android Chrome standalone keeps its edge
stretch under contained overscroll at no cost; iOS runs in Safari
rather than standalone by deliberate choice, so its bounce is the
browser's own and is not independently configurable here. Neither was
device-verified at filing time. Screenshots from the browser checks
will be added here.
