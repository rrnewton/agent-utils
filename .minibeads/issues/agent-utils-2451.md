---
title: 'search-clear-discuss-top-shift: the desk Discuss panel stands about 50 px higher after a search clears than while its results show'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- search
- discuss
created_at: 2026-11-28T06:05:00-05:00
updated_at: 2026-11-28T06:05:00-05:00
---

# Description

Found by the advisory design review of `#2442` search-panel-design-followups item R5 (ui-rules R1-R17), in its screenshot matrix (fixture server, desk 1280x800, light and dark, Discuss open). Already on main before that change; not caused by it.

At a desk with the Discuss panel open over the channel, the panel's top edge stands at CSS y 270 while a search's results show (the search's "Search the server" line at the list's foot) and at CSS y 220 after the search is cleared with Escape: about 50 px higher, the height of that foot line, over the rows the reader was reading. The panel's foot appears to follow the channel composer's top, which rises when the search's foot line goes.

# Design

- First check it against `#2442` R12 (the phone Discuss sheet's top edge jumping as a search clears with the keyboard open): if one cause moves both, fold this into R12 and close it as a duplicate.
- Otherwise: the desk Discuss panel keeps its top edge through a search's clear (it is anchored to something the search's foot line does not move), or the move is held until the reader acts, per ui-rules R12 (the reader's place).

# Acceptance Criteria

- A browser walk (fixture server, never the live server) at 1280x800 with Discuss open: type a search, clear it (Escape, the X, the magnifier, the field emptied); the Discuss panel's top edge moves at most 0.5 px across the clear, per frame. Fails on main before the fix.
- Owner-UI matrix for the change: desk light and dark, Discuss open, before and after, with BUILD.txt.

# Notes

- Filed at the coordinator's request from the `#2442` R5 lane's design review (its crops of the panel's top in the two states, desk light, Escape). Low priority; check against `#2442` R12 before staffing.
