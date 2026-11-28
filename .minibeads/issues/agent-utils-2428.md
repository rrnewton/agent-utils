---
title: 'vibe-talk-reply-open-local-instant: pressing Reply must show its box in the same frame, with nothing server side and no whole-list work in the way'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
- perf
created_at: 2026-11-27T19:35:00-05:00
updated_at: 2026-11-27T23:10:13-05:00
---

# Description

The owner, 2026-11-27 7:24 PM ET, verbatim: "Pressing reply and popping open the reply window should be 100% LOCAL AND INSTANT. It should not depend on anything server side."

Owner-visible: on a long channel (the owner's lists run to about 100k px of rows, with windowed rendering off as today), the reply box comes up late after the tap.

The open path (view/channel/replysession.ts openReply, dockReply and their helpers) is synchronous in code. What the tap costs at that scale has not been measured:
- any network request that gates the box;
- any synchronous re-render of the whole list;
- any layout read across all rows (or a forced layout after the box is moved into the list) before the box shows.

# Design

- Trace the Reply tap on a fixture at the owner's scale (about 100k px of rows, windowed off): the script time from the tap to the frame that shows the box, the long tasks, the layout and style recalculation counts, and every request made between the tap and the box.
- Name each cost (a network wait, a whole-list re-render, a layout read across rows) and remove it from the path to the first frame. Work that can wait, waits until after the box is painted. Nothing server side gates it.
- Coordinate file overlap with the client-performance lane (windowed default, selectionchange, JSON parsing).

# Acceptance Criteria

- At the owner's scale, phone and desk: the box is visible in the first frame after the tap, with under 16 ms of script between them (counted, not timed, where a count can stand: layout and recalculation events, rows touched).
- No network request is issued before the box shows, or needed for it.
- A walk asserts both. Before and after traces, with BUILD.txt.

# Notes

Filed at the coordinator's request (owner P0, fast track), issue file only.

Landed e2baf4c3; deploy pending (it waits for the launch-hold release). It closes after the deploy and its post-deploy guards.
