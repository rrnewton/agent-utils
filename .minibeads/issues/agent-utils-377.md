---
title: 'compact-collapsed-rows: a collapsed message''s header fits on one line, with Reply and Done once it is opened'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T18:49:58.836382185+00:00
updated_at: 2026-10-19T20:37:18.934410966+00:00
closed_at: 2026-10-19T20:37:18.934410836+00:00
---

# Description

[opus 5.5] The owner, 2026-10-19, on a phone in All: a collapsed message's header took two lines. It read the name, the time, Reply, a Thread(32) pill bigger than the buttons beside it, and the ⋯ menu, and Done or Unarchive wrapped alone onto a second line. "We want collapsed messages to be tight."

Wanted:

1. On a 393 px phone, a collapsed row's header is one line: the name (cut to a prefix with an ellipsis when it does not fit), the time, a compact Thread tag and ⋯.
2. Reply and Done/Unarchive are not shown on a collapsed row and appear once it is opened. ⋯ stays on the collapsed row. A swipe still archives or un-archives a collapsed row.
3. The Thread tag is the same height and type size as Reply, ⋯ and Done, and keeps its thread's colour.
4. On an opened row, the controls do not wrap needlessly: no button sits alone on a line.
5. At a desk the same rule applies wherever the list is narrow (the two columns of a smaller window). A wide list keeps every control on every row, on one line.
6. Keyboard and screen readers can still reach Reply and Done: Enter opens the row, and r/e still act on the selected row. A hidden control is out of the accessibility tree, not just invisible.

Acceptance: a browser check at 393x852 and 1280x800 (one and two columns, and two columns at 1024x768) that fails on the old page. It covers collapsed rows on one line without Reply/Done, long names cut with an ellipsis, Done and Unarchive on opened rows, tap and Enter, swipe, and the Thread tag's size and colour. Its negative control requires an expanded row to still show Reply and Done.
