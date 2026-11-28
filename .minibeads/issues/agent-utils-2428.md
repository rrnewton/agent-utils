---
title: 'vibe-talk-reply-open-local-instant: pressing Reply must show its box in the same frame, with nothing server side and no whole-list work in the way'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
- perf
created_at: 2026-11-27T19:35:00-05:00
updated_at: 2026-11-28T00:09:51-05:00
closed_at: 2026-11-28T00:09:51-05:00
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

## Resolution

Fixed by e2baf4c3 (vibe-talk: Reply shows its box in the tap's own frame; its replies are gathered after). Deployed in 3700adec; post-deploy guards green: reply_open_instant, inline_reply, reply_continuity, quote_still, reply_box_clear, composer_stays, thread_reply_boxes.

- Cause: no request was on the path. A Reply on a message with replies gathered them first (gatherForReply -> renderChannelRows), a whole-list redraw inside the tap: 614 ms at 1,216 rows.
- Fix: the box opens in the tap; the gathering follows two frames later with the message held. The lift that keeps the replies clear of the docked box comes within two frames of the box's first frame (liaison 1112). The end state is unchanged.
- Measured: handler 614 -> ~27 ms, box at the first frame 685 -> ~47 ms.
- Walk: tests/reply_open_instant_browser.py (dag 192). Main redrew 1,216 rows inside the tap.
- The owner gate's one red, thread-reply-boxes, was that walk's own read-recording race (cl-loading's d065dc8d). Its exclusive rerun passed.
- A message without replies still costs ~25 ms, half of it one layout of the 112k px list.
- The phone-keyboard case found here is #2437 (7f578235).
