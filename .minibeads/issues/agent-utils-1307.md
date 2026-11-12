---
title: 'doc-fetch-visible-viewport: Fetch visible fetches only the rows on screen, with a progress count'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
created_at: 2026-11-11T15:28:12+00:00
updated_at: 2026-11-12T12:28:47.777123465+00:00
closed_at: 2026-11-12T12:28:47.777122764+00:00
---

# Description

[watermelon-full-preview] Owner reply 363 (2026-11-11 10:21 AM ET): "Fetch visible... button changes to 'Fetching...' but no other notion of progress... I THOUGHT pressing this button would look like an asynchronous series of button presses (with some concurrency level) reverse chronological filling my screen. Instead it took QUITE a long time... should say 'Fetching (7/12)'... it fetched far MORE than the 'visible' 11 docs on the screen... ~8 screens worth".

Verified on github/main: `fetchVisible` in `vibe-talk/web/src/view/documents/list.ts` is `fetchMany(visibleRows())`, and `visibleRows()` is every loaded row passing the bucket/starred/hidden filters, not the rows in the viewport. The list loads 100 rows at a time (`LIST_PATH` limit 100, older pages via `before_ms`), so "visible" means up to 100 rows. `fetchMany` is strictly serial, re-renders per row, and ends with a full `refresh()`; `index.ts` shows only "Fetching…" with no count.

# Design

- "Fetch visible" means the unfetched rows whose cards intersect the viewport (the scroller's visible band), newest first (reverse chronological, as the owner expected).
- Fetch with a small concurrency pool (about 4 in flight). Each card updates as its own fetch lands; no full-list refresh at the end (patch the fetched rows in place).
- The Fetch button reads "Fetching 7/12" (landed/targets) while the run is in flight, then returns to "Fetch"; the completion status still says how many documents were fetched.
- Optionally offer a separate "Fetch all unfetched (N)" menu item for the old whole-loaded-list behaviour, named for what it does.
- Tests wait on events/conditions, never wall-clock budgets (owner rule 346); browser-check at phone (393x852 touch) and desktop (1280x800).

# Acceptance Criteria

- With more loaded rows than fit on screen, Fetch visible fetches only the on-screen unfetched rows, newest first, at most about 4 at a time.
- The button shows a running "Fetching done/total" count and each card flips to fetched as it lands, with no full refresh redraw at the end.
- Documents view/browser tests covering the new scoping, ordering, concurrency cap, and progress label pass at both sizes; the fix is gated at owner tier and landed on its own.

# Notes

Owner reply 363, fast-track. Filed by mu-docfetch before implementation; the fix lands separately on mu-docfetch/doc-fetch-visible-viewport. Sibling fixes: #1306 doc-star-gold, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress. Stay in `vibe-talk/web/src/view/documents/` (and tests); do not edit `web/voice.js` (#624 in flight).
