---
title: 'vibe-talk-discuss-since-field-clips-meridiem: the Discuss since-time field cuts "AM" to "AI"'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:37:00-05:00
updated_at: 2026-11-27T04:05:00-05:00
closed_at: 2026-11-27T04:05:00-05:00
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

## Resolution

[claude-opus-5-5, lane cl-pagenits] Landed in 63d0e0af (2026-11-27 3:50 AM ET, liaison LAND NOW; owner gate on 92f27e94 fully green, 226/226 steps, no NO_RESULT; re-checked once on the exact replayed tree fa01f7d3: discuss_browser with the sweep, tests/js). The since field is sized for a whole 12-hour time beside its picker button (`calc(8ch + 2.75rem)`, `calc(8ch + 2.5rem)` in the phone rule), and the #1524 container query drops the whole since part at 500px (was 480; at 481-500 the field ran a few px past the panel's edge). tests/discuss_browser.py measures the drawn time plus the button's room inside the field, and sweeps the Discuss widths 440-640 px holding the row on one line with every part inside the panel (red with 480 at 504 px). The first version (edc48377) was superseded before landing.
