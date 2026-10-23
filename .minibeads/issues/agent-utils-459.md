---
title: 'pack-keeps-card-still: a card packed or unpacked in the Threads view stays exactly where it is on screen'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-23T14:51:03.243544781+00:00
updated_at: 2026-10-23T18:38:10.309521495+00:00
closed_at: 2026-10-23T18:38:10.309521325+00:00
---

# Description

[opus 5.5] The owner: packing a thread card high on the screen near the foot of the list makes it jump to the bottom, because the list clamps at its end and will not show blank room past it. Wanted: the card acted on (click, the triangle, ^, Enter, o; and Pack all / Unpack with a clear anchor card) keeps its exact on-screen position. Where the content below shrinks and the list would clamp, temporary room past the end (overextension) is added so nothing above moves. The room is one-way: consumed as the reader scrolls up, never re-entered by scrolling down, shrinks as new content fills it, never accumulates beyond what is needed. Normal scrolling keeps the #338 column-overscroll stopping point; a new arrival while overextended does not yank the view.
