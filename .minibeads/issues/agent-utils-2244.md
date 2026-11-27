---
title: 'vibe-talk-discuss-hours-field-scale: the Discuss hours field switches to days past a day or two and never clips'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T10:15:00-05:00
updated_at: 2026-11-27T04:05:00-05:00
closed_at: 2026-11-27T04:05:00-05:00
---

# Description

Found while reviewing the screenshots of #2233 vibe-talk-glossary-popup: with Discuss open, the Summarize row's hours field read "37920.6 hours" (the screenshot server's seeded messages are years old), and on the desk the field showed only "37920", the decimal clipped.

This is not only a fixture artifact. The field shows the hours since the oldest unread message inside the bound (all unread by default), floored to one decimal and always printed with one (`hoursFloor1` / `formatHoursFloor1` in `web/src/view/catchup/panel.ts`), with no cap and no change of unit. A real channel whose oldest unread is about 42 days old or more shows four digits plus a decimal ("1008.0"), and one about 417 days old or more shows five ("10000.0"). The field is sized for short values (`#discuss-hours`: width 3.4rem, min-width 8ch on a desk; 2.7rem on a phone), so on the desk such a value is clipped. It predates #2233 (the bound inputs come from #1817 and #1524).

# Design

- Past a day or two, the bound reads in days (for example "41 days" or "41.5 days"); under that it keeps hours. Typing in the field keeps working in whichever unit it shows, and the linked count and since-time ends follow as they do now.
- Large values drop the decimal (whole hours or days), since a tenth of an hour means nothing at that scale.
- The field is sized to its value (or given room for the longest value it can show), so no value is ever clipped, on a phone or a desk, in either theme.

# Acceptance Criteria

- Page tests: a bound whose oldest unread is minutes, hours, about 42 days and about 417 days old shows a sensible unit and value each time; typing a value in either unit sets the same bound as today; nothing is clipped (the field's scroll width is not wider than its client width).
- Screenshots of the Discuss head with a years-old bound on phone and desk, light and dark, showing the whole value.

# Notes

Filed at the coordinator's request as a side finding of #2233; not part of it.

## Resolution

[claude-opus-5-5, lane cl-pagenits] Landed in 22f69412 (2026-11-27 12:40 AM ET, liaison LAND NOW on an owner gate). Past 48 hours (`BOUND_DAYS_FROM_HOURS`) the bound's field reads in days (floored to one decimal under 10, whole from 10) with its unit word beside it, and what is typed is taken in that unit; the field is as wide as its value plus its room (`--discuss-hours-chars`), never under 8ch. tests/js/view/catchup.test.mjs (two tests), the TUNING_BANDS pin, and tests/discuss_browser.py (the step accepts days; a seven-character value fits whole).
