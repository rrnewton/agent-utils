---
title: 'two-column-full-width: in two columns every row but the owner''s spans its column, with no dead gutter between them'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T14:44:44.427096758+00:00
updated_at: 2026-10-19T15:20:33.352779849+00:00
closed_at: 2026-10-19T15:20:33.352779758+00:00
---

# Description

[opus 5.5] The owner asked a while ago for full-width messages from the coding agent in two-column mode, and the request never reached an agent. On a wide desk (about 2000 px) in two columns, Main sits on the left and the agent's long boxed reply stops about 160 px short of the right column; that strip is empty but for the floating search button. Each column holds its rows to the single-column reading width, centred in the column, and the speakers' desk-sized insets cut the agent's tile shorter still.

What is wanted:

1. In two columns, the rows of other people and of the agent (its long boxed replies) use the full width of their column, up to the divider. The owner's own rows keep their partial width, set in from the left, so they stay visually distinct: the rule the narrow phone column already uses, applied per column. The owner, correcting the first version of this issue (2026-10-19): "MY messages are still partial width to make them visually distinct. The agents usually-long messages are full column. That's how we previously did it on narrow phone columns."
2. No dead gutter between the columns: the search button sits in the band over the head of the left column's list, at the divider, without reserving a strip of its own beside the rows.
3. The right column's rows (its heading, the thread list's cards and a thread's messages) span their column too.
4. Single-column desktop and the phone are unchanged; a single column's reading line keeps its maximum width.

Page tests (the owner's rows NOT full width, everyone else's full width, with negative controls), plus Chromium screenshots before and after at 1920x1080 and 2560x1080 in two columns, with a long agent message and an empty thread list.
