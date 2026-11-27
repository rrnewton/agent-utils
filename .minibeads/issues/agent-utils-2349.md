---
title: 'vibe-talk-discuss-since-field-clips-meridiem: the Discuss since-time field cuts "AM" to "AI"'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:37:00-05:00
updated_at: 2026-11-26T22:37:00-05:00
---

# Description

Found by the advisory design review of #2244 vibe-talk-discuss-hours-field-scale (its screenshot matrix, scenes 73 and 74, light and dark): in the Discuss panel's Summarize row ("N unread, X hours since HH:MM"), the since-time field is too narrow for its own value with a 12-hour clock, so "04:22 AM" shows as "04:22 AI" (the "M" is cut by the field's clock icon and edge). It is there on main before #2244 and is not caused by it. A value the field holds and does not show is a silent cut (R6 in vibe-talk/docs/ui-rules.md).

The field is `#discuss-since` (`type="time"`), sized in web/styles/voice/22-discuss.css (5.6rem on a phone); the browser draws the time and its picker icon inside that box, and a 12-hour value with its AM/PM needs more room than a 24-hour one.

# Design

- Give the since-time field room for its widest value in the reader's clock (HH:MM plus AM/PM and the browser's picker icon), on a phone and on a desk, in both themes, so no part of the time is ever cut; a one-line width change in 22-discuss.css, sized in ch so it follows the type size.
- Keep the row on one line where it is one line today; where the head cannot hold it, the existing container query still drops the whole "since HH:MM" part (#1524), never a piece of the time.
- Touch nothing else in the row (the count and the hours or days field from #2244 keep their sizes).

# Acceptance Criteria

- A browser check at 412x915 and 1280x800, light and dark, with a 12-hour time in the field: the field's scroll width is not wider than its client width, and the whole "HH:MM AM" text box lies inside the field next to its icon.
- Screenshots of the Discuss head (scenes 73 and 74, both themes, phone and desk) before and after, each folder with its BUILD.txt.
- A fresh advisory design review over the after shots reports no R6 cut in the row.

# Notes

[claude-opus-5-5, lane cl-pagenits] Filed at the coordinator's request from the #2244 review (2026-11-26); the liaison wants it fixed in the page-nits list as its own small change with its own evidence and owner gate.
