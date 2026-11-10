---
title: 'keyed-row-patching: redraws patch rows in place instead of replacing them'
status: open
priority: 0
issue_type: feature
assignee: mu-scroll
created_at: 2026-11-10T17:41:25.493620617+00:00
updated_at: 2026-11-10T17:41:25.497123660+00:00
---

# Description

Device evidence (owner's auto-sent scroll traces, 2026-11-10, #1144 part 2b + #1208): of 4204 trace entries, 3400 are kind=mutate - a row removed and re-added for the same message id. Reads and redraws replace row nodes wholesale (replaceChildren with freshly built rows) instead of patching them in place, so every redraw re-derives every row's height, moves what the reader is looking at by small amounts again and again, and takes any text selection with it.

#1209 selection-survives-updates keeps the selection across those rebuilds and skips the provably unchanged cached redraw; this issue removes most of the rebuilds themselves: the redraw paths (the projection, the newest-page landing, the read landing, the cached redraw) patch in place - a row whose message is unchanged keeps its node, changed rows are re-dressed or replaced, and rows are moved, inserted and removed to match the incoming order.

Success looks like: an auto-sent trace over the same reading session shows mutate entries only for rows that actually changed; heights stop being re-derived for unchanged rows; selections survive by construction, not by restoration.

# Design

In the redraw bodies (readcore's projection/newest/landing/cached redraws and channelrows' row building): index drawn rows by their message ids (data-ids), and for each incoming group, reuse the drawn node when the message(s) it stands for are unchanged (same ids and the same rendered stamp - text, fold and reaction inputs the row was built from), re-dress it in place otherwise (renderChannelRows already dresses in place), replace only where dressing cannot say it, and move/insert/remove nodes to the incoming order. The scroll/selection keeping around the redraws stays as it is.

Boundary with mu-stable's D4 (one correction per frame): D4 owns correction count and timing in the read landing plumbing; this owns correction size (row node identity) in the redraw bodies. Either order composes.

Tests: node identity kept for unchanged rows across a landing and a projection (fixture page tests + browser walk), changed rows still redrawn, and the typing-scroll walk's selection case passing by construction.

# Acceptance Criteria

- A landing or projection over unchanged messages keeps every unchanged row's DOM node (identity asserted in tests); only changed/new/removed rows are built, moved or removed.
- A text selection in an unchanged row survives a landing without any restoration (the node never left).
- The typing-scroll walk and page suites stay green; a device trace afterwards shows mutate entries only for changed rows.
- Own owner-tier gate, targeting the morning after filing (owner fast lane).

# Notes

Follow-up to #1209 selection-survives-updates (the stopgap) and #1144 scrollback-typing-jump; device evidence from #1208 scroll-trace-layout-shift traces. Boundary with mu-stable D4 agreed 2026-11-10.
