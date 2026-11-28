---
title: 'search-escape-clear-moves-found-message: pressing Escape to clear the search moves the found, highlighted message on screen'
status: open
priority: 0
issue_type: bug
assignee: cl-searchux
labels:
- vibe-talk
- search
- owner-ui
- scroll
created_at: 2026-11-27T21:45:00-05:00
updated_at: 2026-11-27T21:45:00-05:00
---

# Description

The owner, 2026-11-27 7:09 PM ET: "...when I find a message and I press escape to clear the search. The highlighted message doesn't stay put on screen."

What happens: the reader searches, finds a message (it is on screen and highlighted as a hit), and presses Escape to clear the search. The list goes back to the whole channel, and the found message moves on screen, or leaves it. Owner rule 370 (ui-rules R12): the row being read never moves. When the search clears, the message the reader found is the row being read, so it must keep its place on screen.

Hypothesis from reading the code (a local repro will confirm or correct it): clearing the search is the inverse of the transition `#2346` fixed. The rows the search hid come back above and below the found row, the search's own line and chips leave, and the list's height changes in the next frames. A scroll keeper then restores by some other mark than the found row's top: the foot (a pinned mark), a row the search had not shown, or a held room (`holdRowAt`) left from the search that the clear drops.

# Design

- When the search clears (Escape, the glass, or clearing the field), the row the reader was looking at keeps its top on screen: the highlighted hit when one is on screen, else the shown row nearest the reading line. Every change the clear makes (rows coming back, the search line and chips leaving, a held room released, placed server-hit cards removed) is laid out around that row, by its top, never by following the foot.
- The clear names its cause to the scroll keeper (`#2354`'s `ScrollCause`), so no restore in the following frames follows the foot from it.
- A reader who cleared the search with no hit on the screen (or at the head of the search) keeps today's behaviour, unless the same row-by-top hold is cheaper and keeps what they were reading.

# Acceptance Criteria

- A local browser walk (fixture server, never the live server): search, scroll a hit into view mid-list, press Escape; a per-frame trace of the hit's top from the frame before the key until 60 frames after the last change moves it at most 0.5 px (or 1.0 px where placement is on whole pixels, stated in the evidence), on a phone (412x915) and a desk (1280x800). The walk fails on main and passes with the fix.
- The same trace for clearing with the glass and with the field emptied.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, with before and after screenshots and BUILD.txt; the advisory design review.
- `#2346`, `#1734`, `#2260` page tests, `search_scroll_browser.py` and the other search walks unchanged.

# Notes

- Owner UI, fast-track (owner 7:09 PM ET 2026-11-27). Filed at the coordinator's request (issue file only); the same owner message's second half is `#2424`.
- Related: `#2346` the search-landed reader follows the foot (entry side), `#2354` scroll causes, `#2230` the reading row stays put, `#2422` search hits vanish after a redraw, `#2426` loading program.
