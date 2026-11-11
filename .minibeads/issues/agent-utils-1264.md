---
title: 'documents-scroll-landings: first open at the top, Back to the bottom'
status: open
priority: 1
issue_type: bug
assignee: mu-docscroll
created_at: 2026-11-11T04:15:00+00:00
updated_at: 2026-11-11T04:15:00+00:00
---

# Description

[watermelon-full-preview] Owner replies 355 and 356 (2026-11-10 ET):

(A) Reply 355: "Also when I click on a document in the list for the first
time it doesn't always go to the top of the document". Opening a document
for the first time must land at its top (the shared reader scroller at 0)
after the content renders, including when the document body is fetched
late. A later re-open may restore where the reader left it, where that is
the existing design (the `#1082 documents-back-chain` chain already
restores a previous document's place on Back).

(B) Reply 356: "Also when I come back to the document list sometimes it
is in the middle of the scroll back not the bottom". Returning to the
list (Back, or switching to Documents) lands at the BOTTOM (newest;
`#1047 documents-list-order` / `#1079 documents-chronological` order),
unless the user had deliberately scrolled up in the list before opening
a document; then Back returns exactly there (the
`#1047`/`#1081`/`#1082 documents-back-chain` Back contract).

Diagnosis (vibe-talk page): the list and the reader share one scroller
(`#scroll-area`). Nothing records the list's place when a document opens
from the list, and nothing lands a first-opened document at its top: the
reader draws the fetched body over whatever scroll position the scroller
happened to hold, and Back hands that same position back to the list's
refresh, which re-applies the reader's offset to the list. Late content
(body fetched on open; list rows fetched on return) makes both landings
intermittent. `#scroll-area` does not yet carry `overflow-anchor: none`
(`#1251 scroll-single-owner` routes its holders through the owner
first), so native anchoring also moves these landings; the fix must not
rely on it.

# Design

- Through the Documents view's existing scroll surface only: no new
  direct scroll writes outside the scroll owner (`#1251`'s guard,
  `vibe-talk/tests/scroll_writer_guard.py`, path P4), and no edit to the
  `#624`-frozen files (scrollkeep.ts, folding.ts, foldkeys.ts,
  keyboard.ts, frame/scroller CSS). If the right fix needed a frozen
  file, it would stop and be named to the coordinator instead.
- First open: when a document opens as a new chain entry, land the
  shared scroller at its top after the reader has drawn the fetched
  content (the existing restore surface in
  `view/documents/index.ts`). A `#fragment` open still scrolls to its
  heading after that; Back to a previous document still restores that
  document's saved place.
- Return: record the list's place when a document opens from the list
  (its offset, and whether it was parked at the bottom). Back to the
  list re-applies that place after the rows render: the bottom when it
  was at the bottom, the exact offset when the user had scrolled up.
  A plain switch to Documents keeps landing at the bottom on first
  entry and keeps the place the page's per-view restore already holds.

# Acceptance Criteria

- Browser tests at phone (393x852) and desktop (1280x800), including a
  SLOW-FETCH case (the document body and the list rows delayed): a
  first open lands at the top; Back lands at the bottom; Back after
  deliberately scrolling up in the list returns exactly there.
- Focused unit tests cover the same three landings over fakes with
  delayed answers.
- The scroll-writer guard reports no new direct writer in the
  documents path (P4 inventory does not grow).
- Selected validation passes locally before the owner-tier gate; the
  gate passes or pre-existing reds are proven on a clean github/main
  checkout.

# Notes

Owner replies 355/356, 2026-11-10. Slot docscroll, branch
mu-docscroll/documents-scroll. Related: #1047 documents-list-order,
#1079 documents-chronological, #1081 documents-relative-links,
#1082 documents-back-chain, #1224 mention-jump-in-view,
#1229 keyed-row-patching, #1251 scroll-single-owner, #624
voice-page-modularity. Parallel Documents lanes (list cards, star/hide,
search) coordinate through the coordinator.
