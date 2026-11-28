---
title: 'search-clear-two-columns-found-hold: with two columns up, clearing a search keeps the found message in the channel list, and the right column still follows a reply in its thread'
status: closed
priority: 0
issue_type: bug
assignee: cl-searchux
labels:
- vibe-talk
- search
- owner-ui
- scroll
created_at: 2026-11-28T01:45:00-05:00
updated_at: 2026-11-28T10:20:00-05:00
closed_at: 2026-11-28T10:20:00-05:00
---

# Description

`#2423` search-escape-clear-moves-found-message keeps the message the reader found where it stands when a search clears (Escape, the X, the magnifier, the field emptied; owner rule 370, R12). Its first owner gate caught a break with two columns up: a search typed and closed in the right column's cards left a found row held in the channel list on the left, and the thread opened in the right column next no longer followed an arriving reply at its foot (`column_overscroll_browser.py` FOLLOW: "a reply arriving with the reader in the room was not followed as at the foot", 3 of 3 on that head, 0 of 3 on main). Keeping the found row only in the left column did not clear it, nor did skipping a reader whose last pane was the right column (a reader who only turned the wheel there is not seen as working in it). So `#2423` landed with the found message kept only with one column on screen; with two columns up the list keeps today's close.

The owner reads on a wide desktop (the installed page, two columns), so the owner's ask ("when I find a message and I press escape to clear the search, the highlighted message doesn't stay put on screen") is not yet met there. Liaison 1140: owner-UI priority, right after `#2424`.

# Design

- Find why a found-row hold in the left list stops the right column's thread follow: which shared state the clear leaves behind (the filter place's hold, the scroll owner's rooms past the foot and floor, its reader-input stamps, the follow state the arrivals read) and which arrival path reads it for the right column.
- Make the hold state per column, so the right column's follow and the list's found-row hold are independent: a hold in one column never vetoes or moves the other.
- Then keep the found message with two columns up too: Escape (and the X, the magnifier, the field emptied) keeps the found message in the channel list where it stands, with the right column keeping its own close.

# Acceptance Criteria

- `search_clear_stays_browser.py` gains two-column walks (desk 1920x1080, light and dark): the found message in the channel list keeps its top within 0.5 px per frame through the clear (Escape, X, glass, emptied; landed, scrolled, highlighted), failing on the `#2423` head where two columns keep today's close.
- Its two-column walk (a search closed in the right column, then `column_overscroll_browser.py`'s thread walk there) and `column_overscroll_browser.py` (desk-1280 and desk-1920, FOLLOW included) pass.
- Owner-UI matrix at desk widths with two columns, light and dark, Discuss open, with before and after screenshots, BUILD.txt and the advisory design review.

# Notes

- Step 1 (liaison 1151, 1152): the right column's box, and its pane while a reply is open in it, keep no room for the left column's chips (`--scroll-tools-clearance`, shared with the channel's box; the chips float over the left list only). That room was what let `column_overscroll_browser.py`'s FOLLOW pass on main: a reply arriving with the reader in the right column's room past its foot left them short of the new foot until, a frame later, the left column's Newest pill hid and shrank the right foot under them, and the foot keeper's size-change follow did the last pixels (`#2444`'s intermittent 4 px miss when that timing slipped). The FOLLOW expectation is rewritten per owner rule 370 (`#2230`, liaison 1152), replacing `#338`'s "followed as at the foot": the row being read keeps its top (<= 0.5 px) at every frame, the reply shows below it, and the jump offers it where any of it is below the fold. Reached at the foot with the room closed (ARRIVE) the row being read holds as before.
- Step 2: keep the found message with two columns up (stacked on step 1): `search_clear_stays_browser.py` gains two-column walks at a desk (1920x1080, a fixture with one thread so the layout button is offered): the found message in the left list keeps its top through Escape, the X and the magnifier after the entry landed, after a scroll in the results, and when tapped, and a search typed and closed with nothing found still puts back the row read before it.

- Filed at the coordinator's request (issue file only), from the `#2423` first owner gate's catch and its scope correction (liaison 1133, 1140).
- Related: `#2423`, `#2424`, `#2442`, `#2444` (a separate intermittent 4 px FOLLOW miss in the same walk), `#2230`, `#2346`, `#2354`.

## Resolution

[opus 5.5] Landed in 90ff376b (step 1) and 022cf893 (step 2) on 2026-11-28 at 9:47 AM ET, on the coordinator's LAND NOW after the liaison's OK (1151, 1152, 1156). Gated head 7921b0a0 on 9bf112c0, after `#2426` R3 and `#2425` landed; the rebase left both commits identical (range-diff "=", same patch-ids). The owner gate there had four browser walks red at load1 300-350: fold-steps, reload-place, search-typing-stays and view-switch. The change touches none of them: three are phone walks, where it changes nothing, and one is a desk walk with one column. All four passed on the solo rerun (load1 about 127). The search-typing red's unreproduced 110 px move is `#2463` search-typing-foot-jump.

With two columns up, clearing a search keeps the found message in the channel list, as `#2423` does with one column. The right column's box and reply pane no longer keep room for the left column's chips. A reply arriving with the reader in the right column's room past its foot moves nothing: the row being read keeps its top, and the reply shows below it (owner rule 370, replacing `#338`'s follow).

Evidence (fixture server; desk 1920x1080 with two columns, light and dark, Discuss open):
- `search_clear_stays_browser.py`: 51 walks, max 0.48 px per frame, including the two-column walks; the light and dark two-column pass, 12 walks, max 0.48 px.
- `column_overscroll_browser.py` FOLLOW at desk-1920: main 9/10 fail (the timing-luck follow), step 1 10/10 pass; after the rebase 3/3 pass, every negative control failing as it must.
- Before/after pairs, BUILD.txt and the advisory design review are in the lane's evidence. The right column's foot is 40 px lower with the room closed, still clear of the dock.

Live in 022cf893: the coordinator's deploy and its watch both exited 0. Post-deploy checks on the page built from 022cf893 (fixture server only, never the live server), 2026-11-28 10:09-10:20 AM ET, all passed:
- `search_clear_stays_browser.py`: 51 walks, max 0.48 px, two-column walks included (load1 198 -> 156).
- `column_overscroll_browser.py --size desk-1920`, with its negative controls (load1 156 -> 161).
- `search_typing_stays_browser.py` (load1 161 -> 159).
- `fold_steps_browser.py` (load1 159 -> 156).
- `two_column_browser.py` (load1 156 -> 155).

`#2444`'s intermittent 4 px FOLLOW miss came from the same path. It is closed (12bceede) as fixed by step 1: 25 of 25 runs passed at desk-1920 on main 022cf893, 20 of them starting under load1 237.
