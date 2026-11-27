---
title: 'vibe-talk-search-unmatched-count-race: search_unmatched_browser reads the search count once right after a seam press and can catch it mid-change'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-27T01:10:00-05:00
updated_at: 2026-11-27T01:10:00-05:00
---

# Description

vibe-talk/tests/search_unmatched_browser.py went red once on a lane's tree (2026-11-27 ~12:55 AM ET, phone walk, load1 111): "phone: opening a run changed what the bar counts as matched". The check presses the middle seam line, waits for the opened run's rows to show, and then reads `#search-count` once, expecting exactly "4 of 16 loaded". That run took 48 s against about 14 s for a pass. The same tree then passed 8 of 8 runs alone, and unmodified main passed 5 of 5 in the same alternating runs (load1 120-138). The tree changed only where the filter place-keeper puts the reader and the search entry landing's hold, nothing in the count, so the red reads as the one-shot read racing a later count write (for example a server answer refolding the count), not a regression. The step is already on validation/known-red.toml under #2175 ("search waits under load").

# Design

- The check waits for the count to settle on its expected text (a condition wait with a generous backstop, as `walk.until` does elsewhere in the walk) instead of reading it once, or asserts the count against the state that stands once the search has settled.
- Nothing in the page changes for this.

# Acceptance Criteria

- search_unmatched_browser passes 20 runs out of 20 alone at load1 >= 237 (hot host), load1 recorded per run.
- The count check still fails when opening a run really changes the count (a negative control: a page change that adds the opened rows to the count).

# Notes

Filed at the coordinator's request from the #2346 lane's checks (issue file only). Related: #2175 vibe-talk-browser-load-known-red, #2177, #2346 vibe-talk-search-landed-reader-follows-foot.

Cause (2026-11-27, 20 runs at 4-parallel on main plus an instrumented copy recording every count and server-line change): the walk's `server_settled` waited on the scope text, but the busy words ("Searching the server…") are the server line's, so steps 1 and 2 always ran with the server's answer still out (at the seam press the line still said "Searching the server…" in every run). When the answer landed, its older hits folded into the count ("7 found (4 loaded, 3 older)", `#2200`, the page's correct count), so a slow run in which the answer landed between the press and the one read of the count went red. Forcing the answer to land before the read turns the old check red every time. Not a page fault.

Fix (test only): the fixture server holds the lantern answer through steps 1 and 2 (they are about the loaded rows and their gaps), the walk waits for the ask to be out instead of a settle that cannot see it, the count check compares the count after the press with the count read just before it, and the answer is let through and waited for before step 3, which already expects the fold. `server_settled` now counts the server line's busy words too.

