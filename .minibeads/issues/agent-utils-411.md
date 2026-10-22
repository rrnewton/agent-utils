---
title: 'thread-view-row-padding: unpacked thread rows get the message row''s box, so header buttons clear the top border'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-22T02:46:02.087732479+00:00
updated_at: 2026-10-22T02:48:13.165882839+00:00
closed_at: 2026-10-22T02:48:13.165882739+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: "My message cards look a little more cramped in thread view. The Reply '...' and Done buttons are touching the top border of the message box and this is not happening in the All view with the same messages rendered." Measured in Chromium at 1280x800 and 393x852 over the two-column fixture: All's rows, a thread drilled in and the right column's thread rows all put the header's buttons 10.6px under the row's top edge; the rows Unpack all (#404 threads-unpack-all) draws under the Threads view's cards put them 0px under it. Those rows live in ul.side-cards, not a .messages list, so web/style.css's '.messages li' box (padding 0.6rem 0.7rem, the 1px hairline, the radius) never reached them: padding, border and radius were all 0.

# Acceptance Criteria

A row unpacked under a card has the same box as a row of All (padding, hairline, radius) from the same rule, and its header buttons sit at least as far below its top border as the same message's in All, at 1280x800 and 393x852. The bridge between unpacked rows still joins them. Page test and Chromium layout check, failing before.
