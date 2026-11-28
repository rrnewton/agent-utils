---
title: 'search-escape-clear-moves-found-message: pressing Escape to clear the search moves the found, highlighted message on screen'
status: closed
priority: 0
issue_type: bug
assignee: cl-searchux
labels:
- vibe-talk
- search
- owner-ui
- scroll
created_at: 2026-11-27T21:45:00-05:00
updated_at: 2026-11-28T04:20:00-05:00
closed_at: 2026-11-28T04:20:00-05:00
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

- Scope (liaison 1116, 2026-11-27 10:3x PM ET, agreeing that the owner's ask overrides `#1968`): closing the search keeps the found or highlighted message put only when the reader landed on a result (the `#1734` entry landing took them to the newest hits, or they moved in the results themselves) or highlighted one (the selection: a desk pointer's light, a tap, the keys). A query typed and closed with nothing found keeps today's behaviour: the row read before the search comes back where it stood (`#243`'s hold, and `#1968`'s restore where the entry had moved them and no result is shown).
- `#1968` change: its close used to put back the parked place after an entry landing; now that close keeps the found result, and the restore runs only for a close that kept no shown result. `search_close_browser.py` checks the landed case against the found hit, and the parked case as before.
- Scope correction (liaison 1133, 2026-11-28 ~12:50 AM ET, after the first owner gate's catch): the found message is kept in the channel list only, with one column on screen. The right column (its cards, a thread) keeps its own close, and with two columns up the list on the left keeps today's close too. The first head kept a found row in the left list for a search typed and closed in the right column's cards, and the thread opened there next no longer followed an arriving reply at its foot (`column_overscroll_browser.py` FOLLOW, 3 of 3 on that head, 0 of 3 on main and on `#2427`). Skipping only the right column, or only a reader whose last pane was the right column, did not clear it (a reader who only turned the wheel there is not seen as working in it), so two columns keep today's close until the interaction is understood: a follow-up. `search_clear_stays_browser.py` now walks that case (two columns: a search closed in the right column, then the gate's thread walk there).
- Found on the way, left to `#2424`: over 200 rows the typing's fast pass (`#1913`) hides rows before the place-keeper measures and notes the typed-at screen again at every key, so the entry's landing (`#1734`) never fires and the typing's hold sits on whichever row the fast pass kept; the landed and typed-closed walks over 260 rows join `search_clear_stays_browser.py` with `#2424`.
- Owner UI, fast-track (owner 7:09 PM ET 2026-11-27). Filed at the coordinator's request (issue file only); the same owner message's second half is `#2424`.
- Related: `#2346` the search-landed reader follows the foot (entry side), `#2354` scroll causes, `#2230` the reading row stays put, `#2422` search hits vanish after a redraw, `#2426` loading program.

## Resolution

[opus 5.5] Landed in a0d58201 (2026-11-28 2:2x AM ET, coordinator's LAND NOW after the liaison's OK; owner gate pass on the gated head bb503027, replayed over `#2427` dbecb0bb); live since 3:50 AM ET in 3aebaeae (watch clean). Clearing a search (Escape, the X, the magnifier, the field emptied) keeps the message the reader found where it stands, by its top, when they landed on a result (the `#1734` entry landing or their own scroll in the results) or highlighted one; a search typed and closed with nothing found keeps today's behaviour; with one column on screen (two columns: `#2445`).

Evidence (fixture server, `search_clear_stays_browser.py`, per-frame trace of the found message's top from the frame before the clear until 60 still frames after): main moved it in 54 of 74 walks (30 px to 19,820 px), the fix in none (worst 0.48 px), desk 1280x800 and phone 412x915, light and dark, keyboard open and closed, Discuss open, 120 and 260 rows; the typed-closed walk keeps the row read before the search on both. The first owner gate (on 9219e82f) caught `column_overscroll_browser.py` FOLLOW (3 of 3 on that head, 0 of 3 on main and on `#2427` alone): the found row held in the left list of two columns left the right column's thread short of following a reply; scoped to one column (liaison 1133, 1140), and the walk gained that two-column case. Advisory design review: no violation introduced; four pre-existing finds filed as `#2442`.
