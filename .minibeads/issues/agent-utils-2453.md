---
title: 'filter-walk-landing-note-floats: the filter walk-back''s landing line floats as the pill over the row it lands on'
status: closed
priority: 1
issue_type: bug
assignee: cl-filternote
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-28T06:10:00-05:00
updated_at: 2026-11-28T13:10:00-05:00
closed_at: 2026-11-28T13:10:00-05:00
---

# Description

Found by the `#2442` R5 lane (search-panel-design-followups, the stale "Filtered out" note), same class as `#2350`, `#2365`, `#2375` and `#2426` R3: a note floated over the very rows it is about.

When a filter change keeps a message older than everything loaded (an old pin chosen under Pinned, then Pinned turned off), the filter place walks back through older pages and, when it lands, says "Here is the selected message." (the reader had chosen it) or "Here is where you were." (they had not). Both are said as the floating status pill, which stands over the list for six seconds, over the row the walk just put the reader on or the rows around it. The landed row standing where the reader left it is the confirmation; the pill hides it. ui-rules R5 (status is a chip, never a sentence of chrome over content) and the owner's decision on `#2314` (floating controls may cover text; floating notes may not).

# Design

- Announce, don't float, exactly as `#2365` and `#2426` R3 did for their landing lines: both sentences are said in the polite live region (`role=status`, `aria-live=polite`) and floated over nothing (the status line's announced-only form, `setStatus(text, { float: false })` from `#2442` R5).
- "Loading earlier messages…" while the walk reads stays as it is (progress the reader is waiting on, at the list's head).

# Acceptance Criteria

- A browser walk (fixture server, never the live server) that fails on main and passes with the fix: an old pin chosen under Pinned, Pinned cleared (Escape at a desk, the Labels drop-down on a phone), the walk lands and says "Here is the selected message."; and an old pin at the head of the Pinned list with nothing chosen, cleared, says "Here is where you were.": on every frame from the clear until the list is quiet, the sentence, while up, floats over no row's text; it is said (in the live region). Desk 1280x800 and phone 412x915.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, before and after, with BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request (routing of the `#2442` R5 design review); fix stacked on the `#2442` R5 commit (same files), landing after it.

## Resolution

Fixed and live in 95b43883 (landed 2026-11-28 12:45 PM ET, on `#2442` R5 458854c6; deployed about 12:50 PM ET with a clean watch): both landing lines of the filter walk-back ("Here is the selected message.", "Here is where you were.") are said with `setStatus(text, { float: false })`, in the polite live region and floated over no row; "Loading earlier messages…" while the walk reads is unchanged. Evidence accepted by the liaison (1166, with the keyboard-open cells declared not applicable: the walk back starts from the Labels drop-down, which takes the focus out of every text field).

- Walk `tests/filter_walk_note_browser.py` (dag step `vibe-talk.check.filter-walk-note-browser`): desk 1280x800 and phone 412x915, chosen and at the head, Discuss open, light and dark; fails on the parent, passes here. The page suite's walk-back test also asserts the line is announced, not floated.
- Gated as 413c7301 (owner priority, path-selected): two reds, both not this change, then both passed on the exclusive rerun. `vibe-talk.check.labels-browser` failed the same way with and without this change (16 of 17 each), filed as `#2483`. `vibe-talk.shots.screenshots-desks` is known red under `#2175`.
- On the landed page after the deploy: `tests/filter_walk_note_browser.py` 8 of 8 walks pass (2026-11-28 1:07 PM ET).
