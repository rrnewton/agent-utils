---
title: 'unpack-row-parity: unpacked thread rows answer the mouse and keys as message rows do everywhere else'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-21T20:17:17.497952628+00:00
updated_at: 2026-10-21T20:17:17.497952628+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21, after trying #404 threads-unpack-all: "It's a good start. But there are facilities from the other views that are not working. I would like it to be consistent. When I mouse over the unpacked messages they do not highlight. I can click to collapse expand but not use a keystroke on the highlighted one. Keystroke ':' does not work to expand/collapse all. I don't want us to have separate code.. I want us to share code as much as possible between views." In the Threads view (Cmd+3) with Unpack all on, a message row under a card is not lit when the mouse moves over it; the arrow keys and j/k skip it, walking card to card; Enter, o, e, Shift+E, r, s, i, the ⋯ menu's Through here and every other key about one message cannot reach it; ; and : say there is nothing to expand or collapse; and Collapse read draws its own kind of run line, which the keys cannot open, rather than the run line every other list draws.

# Acceptance Criteria

With Unpack all on in the Threads view, an unpacked message row behaves as a message row does in Main, All and a thread: the mouse moving over it lights it (data-selected) and selects it; ↓/↑ and j/k move the one selection through cards and unpacked rows in reading order; Enter/o fold or unfold the selected row, e marks it Done and moves on as elsewhere, r replies, and the other per-row keys act on it; ; and : step the unpacked rows' folds; Collapse read draws the same run line (data-read-run) that Enter opens, Hide read leaves read rows out by the same rule as a thread. Through here covers the rows above it in its own thread. Unchanged: cards alone when Unpack all is off, two columns, Main, All and a drill-in. Built from the shared row, selection, keymap and read-run code, not a parallel copy; page tests fail before, with negative controls; a Chromium walk of hover, keyboard selection and :.
