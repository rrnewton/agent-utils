---
title: 'search-copy-with-placed-hits: with server hits standing as cards, the search line says "plus 4 scattered older ones. None match." and the count says "4 found (0 loaded, 4 older)"; both contradict the cards on screen'
status: closed
priority: 1
issue_type: bug
assignee: cl-shardcap
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-27T21:15:00-05:00
updated_at: 2026-11-27T23:00:00-05:00
closed_at: 2026-11-27T23:00:00-05:00
---

# Description

The coordinator's review of #2422 found that the search copy contradicts itself. This is an owner-UI follow-up to #2422.

A quoted phrase matches 4 messages that lie before the loaded window. The server search finds them, and (since #2422) 4 result cards stand in the timeline. Above them, the page says two things the cards contradict:

- The line under the search field reads "Searched 61 messages in this browser, a complete history back to today, 10:00 AM (no gaps), plus 4 scattered older ones. None match."
  - The "4 scattered older ones" are the 4 result cards. `reachClause` in `web/src/present/notices/search.ts` counts them as scattered older rows the browser holds.
  - "None match" is then false: those 4 do match, and they are on screen.
- The count beside the field reads "4 found (0 loaded, 4 older)". The "0 loaded" part says nothing useful and reads as if the results are gone.

# Design

Make the copy true of what the screen shows. Keep it a page-only change, in its own commit after #2422.

- **The search line.** When placed server results stand, the line says the browser found no match in what it loaded, and that the older matches are shown below. For example: "No match in the 61 loaded messages; 4 older matches shown below."
  - The placed results are not counted as scattered rows the browser searched. The sentence is unchanged when no results are placed.
- **The count.** Zero parts are dropped: "4 found (4 older)", not "4 found (0 loaded, 4 older)". A count with both parts non-zero keeps both, for example "5 found (1 loaded, 4 older)".
- No change to how results are found, placed or counted. Only the words change.

# Acceptance Criteria

- With 4 placed results and no loaded match, the line and the count say the sentences above, and no part of either contradicts the cards on screen.
- With loaded matches only, or with no server answer, the copy is unchanged.
- Before and after cells: phone 412x915 and desk 1280x800, light and dark, plus BUILD.txt. The walk fails on main and passes with the change.
- The existing search walks still pass: server hits, typing fast, bucket chips, reach server, and the others.

# Notes

- Found in the review of #2422's evidence: the cards stood, and the copy around them still said none.
- Related: #2422 search-hits-vanish-after-redraw, #2200 search tab counts match cards, #2269 the reach sentence beside server hits, #578 search status clarity, #2426 the loading program.

## Resolution

Landed as a861ead3 on 2026-11-27. The landing replayed 9f599a47, gated on aace00b3, the landed #2422. The deploy waits for the evening's launch-hold release.

**Change, page only.**

- `olderwalks.ts` `historyReach` no longer counts placed server results as "scattered older ones" the browser searched. That count was the root of the contradiction.
- `searchrows.ts`, the line: when the selected buckets' older matches stand below as cards, it reads "No match in the 60 loaded messages; 4 older matches shown below." The #2269 reach sentence keeps its rule: it stays when the server's answer stopped on its budget.
- `searchrows.ts`, the count: a zero "loaded" part is dropped ("4 found (4 older)"). Two non-zero parts stay, and the thread form is unchanged.

**Evidence per acceptance criterion.**

- **The copy with placed results.** The walk checks the count and the line at placement and after the arrival's redraw. It passes 8 of 8 cells and fails 8 of 8 on aace00b3, where the count read "4 found (0 loaded, 4 older)".
  - On a release build, `search_reach_server` reads "1 found (1 older)" and "No match in the 12 loaded messages; 1 older match shown below." The browser count is now 12 rather than 13, because the old count included the result card itself.
- **Unchanged copy elsewhere.** The 11 search walks and the node suite (3494 of 3494) pass. Only the #2269 test's expected count and line changed, and its budget-stopped case still requires the reach sentence.
- **Cells.** Before and after screenshots for phone and desk, light and dark, keyboard open and closed, and BUILD.txt, were accepted by the liaison (1118).
- **Gate.** The owner gate on 9f599a47 passed: validate OK, all 243 steps ok, no NO_RESULT.
