---
title: 'vibe-talk-quote-reply-dock-keeps-post-visible: the docked reply box must not cover the quoted post'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T11:05:00-05:00
updated_at: 2026-11-22T11:05:00-05:00
---

# Description

After #2063 the quoted post holds still when quote-reply opens, but the reply box docks at the foot of the column and covers the quoted post itself when the post sits low on screen, on phone and in the desktop two-column view. The owner cannot see the post they are replying to.

# Design

Cap the dock so the box sits no higher than the quoted post's last line: the quoted post stays fully visible and still, and the box overlaps only rows below it. If the post is too tall for both to fit, keep the post's last lines visible and say so in the test. Compose with #2063's stillness and #2076's top anchor in the same scroll owner.

# Acceptance Criteria

- Frame checks: the quoted post's top stays within 0.5 px, and its bottom is not covered by the box, phone 412 and desktop 1280, windowed on and off; screenshots of the low case on phone and desktop, light and dark.

# Notes

Owner request 297 follow-up; filed by the coordinator.
