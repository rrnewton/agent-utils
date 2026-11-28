---
title: 'search-toolbar-reflow-on-empty: the phone search toolbar moves its buttons when the field is emptied'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- search
created_at: 2026-11-28T06:05:00-05:00
updated_at: 2026-11-28T06:05:00-05:00
---

# Description

Found by the advisory design review of `#2442` search-panel-design-followups item R5 (ui-rules R1-R17), in its screenshot matrix (fixture server, phone 412x915, light and dark). Already on main before that change; not caused by it.

On a phone with the search open and words in the field, the panel's second row holds five filter buttons and the close X, and the field's row holds the count ("15 of 120 loaded"). When the field is emptied, the count goes, the first filter button jumps from the start of the second row up to the end of the field's row, and the other four slide one slot left (about 38 CSS px each), so the buttons move under the reader's thumb at the moment they cleared the field.

# Design

- The panel's buttons keep their places whether the field holds words or not: the count's place is kept (or shown elsewhere) so emptying the field takes nothing out of the rows, and no button changes rows. Measure at 412x915 and 360x800 before choosing.

# Acceptance Criteria

- A browser walk (fixture server, never the live server) at 412x915 by touch: open the search, type a word, empty the field; every button in the search panel keeps its box within 0.5 px. Fails on main before the fix.
- Owner-UI matrix for the change: phone light and dark, keyboard open and closed, before and after, with BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request from the `#2442` R5 lane's design review (its crops of the panel with words in the field and after it was emptied). Low priority.
