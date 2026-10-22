---
title: 'thread-hue-spread: threads drawn next to each other get clearly different colours'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-10-22T02:59:36.071421044+00:00
updated_at: 2026-10-22T03:14:46.173480297+00:00
closed_at: 2026-10-22T03:14:46.173480177+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: three threads whose root messages are consecutive in time were drawn in VERY similar colours. threadHue gives a new thread the midpoint of the widest unused arc. Since #396 thread-color-memory a channel remembers up to 24 hues, so the widest free arc can be as narrow as 15 degrees and a new thread can land a few degrees from the threads drawn just before and after it. The owner asked for consecutive threads to be anti-correlated, or to re-roll a colour that is too similar.

# Design

Keep the widest-arc midpoint when it is at least a minimum hue distance from the thread's neighbours: the threads up to three before and after it in root-message time order that already have colours, and the last few threads given a colour on this page. Otherwise re-roll through a deterministic candidate order (every whole degree, farthest from every colour in use first) and take the first that clears the neighbours; if none does, the one farthest from its nearest neighbour. Never recolour an assigned or remembered thread.

# Acceptance Criteria

Unit tests: three consecutive new threads after 24 remembered hues are at least the minimum apart and from their neighbours; restored hues unchanged; the re-roll order is deterministic. Chromium screenshot of 8 consecutive threads before and after. make validate passes.
