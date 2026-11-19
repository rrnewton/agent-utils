---
title: 'vibe-talk-documents-filter-bar-fixed: the Documents filter row is a fixed bar above the bottom bar, Starred a star icon on the bottom bar'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-19T15:14:54.000000000+00:00
updated_at: 2026-11-19T22:06:27.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI request, 2026-11-19: on the
Documents view the type-filter chip row (All, Markdown, Pastes,
Issues, Other files, Starred) is the last row of the scrolling list
itself. It scrolls away with the list, and on a phone it wraps so
Starred splits onto a second row. In the same state All and Starred
both read as pressed, so the active filter is ambiguous. Related:
#1544 documents-desktop-control-bar (open, the bottom bar's grouping
on desktop), #1265 documents-star-hide (the Starred chip and the
per-card stars), #1176 doc-kind-buckets (the type buckets).

# Design

- The bucket chips move out of the scrolling list into a fixed bar
  (`#docs-filter-bar`) in the dock, directly above the bottom control
  bar: one row, never wrapping, scrolling sideways if it must,
  opaque, visible only while the Documents list (not the reader, not
  another view) is up, and unmoved while the list scrolls.
- Starred leaves the chip row. A star icon toggle
  (`#docs-starred-toggle`) stands in the bottom control bar's pack:
  outline star and muted ink off, filled gold star on, with
  `aria-pressed` and the label "Starred". It composes with the
  chosen bucket exactly as the chip did.
- Pressed chips are unambiguous in light and dark: a pressed bucket
  is filled and carries a check mark, an unpressed bucket is outline
  only, and the mutually exclusive buckets show exactly one pressed
  at a time (Starred is a separate toggle on a different bar, so it
  can no longer read as a second pressed bucket).
- Scope is the layout and the two controls' placement and dress.
  The list-state logic (which rows a filter admits, starred/hidden
  state, scroll landings) is untouched; that is the sibling lane's
  area (#1908), and this lane rebases on whichever lands first.

# Acceptance Criteria

- On phone and desktop, in light and dark, the filter bar stands
  fixed above the bottom control bar while the Documents list is
  scrolled to its middle, and never wraps to a second row.
- The bottom control bar carries the star icon toggle; off shows an
  outline star, on shows a filled gold star, and pressing it filters
  to starred documents composed with the current bucket.
- Exactly one bucket chip reads pressed at a time, and a pressed
  chip is filled with a check where an unpressed chip is outline
  only, in light and dark.
- Page tests at phone and desktop sizes cover the placement, and
  the feature is exercised end to end through the real server with
  before/after screenshots (light and dark, phone and desktop,
  keyboard closed, Discuss closed).

# Notes

Owner screenshots (phone, dark) showed the chip row at the foot of
the list with Starred wrapped to a second row, and All and Starred
both reading pressed over a one-document starred result.

## Resolution

[watermelon-full-preview] Landed 525c8e88 on github/main. The bucket
chips now render into a fixed dock bar (`#docs-filter-bar`) directly
above the bottom control bar — one row, sideways scroll, opaque,
list-only — and Starred is a star icon toggle (`#docs-starred-toggle`)
in the bottom bar's pack (outline off, filled gold on, `aria-pressed`,
label "Starred", CSS-hidden in the reader), composing with the bucket
through `list.setStarredOnly`. A pressed bucket is filled accent with
a check; exactly one bucket reads pressed in light and dark. The
sibling lane's Back/filter walk (#1908) now clicks the new toggle.
Evidence: the shared harness matrix (scenes 73-76 and
83-documents-filter-bar; light and dark, phone 412x915 and desktop
1280x800) and a seeded 12-document real-server walk (mid-scroll with
the bar fixed, star off and on, all four theme/size combinations),
retaken from landed main. The owner gate's only reds were
reload-place and thread-pin-picker, each passing alone three times
and in an exclusive rerun; screenshots-desks scene 59 is a
pre-existing failure on unmodified main under load, waived by the
coordinator. Deployment is the coordinator's batch.
