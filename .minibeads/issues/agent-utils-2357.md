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
