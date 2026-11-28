---
title: 'search-hits-vanish-after-redraw: server search hits shown as cards disappear after a later redraw, leaving a blank results page that reads "4 found (0 loaded, 4 older)"'
status: open
priority: 0
issue_type: bug
assignee: cl-shardcap
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-27T19:27:00-05:00
updated_at: 2026-11-27T19:27:00-05:00
---

# Description

The owner, 2026-11-27 7:12 PM ET: "Yep now the phrase "low urgency" in quotes DOES find the 11/14 message but then ALL RESULTS DISAPPEAR and I end up on a blank results page. "4 found (0 loaded, 4 older)". so they WERE there and then suddenly they disappear and become older?"

What happens: a quoted phrase matches 4 messages in an old part of the channel, and the server search finds them. The 4 results appear as cards in the timeline. Soon after, every card is gone. The page is blank, because no loaded row matches, and the count line reads "4 found (0 loaded, 4 older)".

## Cause (hypothesis from reading the code; a local repro will confirm or correct it)

- The 4 hits are server hits outside the loaded rows, which the search fold (`searchFoldParts` in `web/src/view/channel/olderwalks.ts`) counts as "fresh". The count line words those as "older". They are shown as placed cards: `placeServerHits` inserts card nodes, marked `data-server-hit`, into `#discord-log`.
- `placeServerHits` is memoized on a key. The key is the answer ticket, hit counts, per-bucket lengths, bucket toggles, view, channel and the all-channels flag. When the key is unchanged it returns without placing anything, and nothing else resets the key.
- A later redraw of the list rebuilds `#discord-log` from the store's row groups. Examples are a poll's newest-page read, through readcore's non-older draw and `patchRows` in `web/src/view/channel/rows.ts`, and the windowed render. `patchRows` removes every child that is not one of those groups, so the placed cards go too.
- The next server render finds the same key and returns early, so the cards are never put back. The count line still reads the fold (4 fresh), which gives "4 found (0 loaded, 4 older)".
- The hits were never loaded and did not become older. "older" was the count line's word for them from the start: they lie beyond what is loaded.

# Design

- A search result that is shown, or was loaded for the search, is pinned while the search is open. No poll, redraw or window trim may remove it, and its counts never move from loaded to older.
  - The redraw keeps placed hit cards. Placement rebuilds them whenever any card it placed is no longer in the list, not on the memo key alone.
  - Check whether any trim of the held messages can turn loaded hits into fresh ones while a search is open. If one can, the same pin covers it.
- Keep the change minimal and compatible with the sparse-residency design of the loading program (#2426): search hits become pinned residents there later.
- If it is cheap, the count line distinguishes results not yet placed from results that are older than the loaded rows. Otherwise note it for #2426.

# Acceptance Criteria

- A local browser walk (fixture server, never the live server): 4 messages holding the quoted phrase, dated well before the loaded window. The search places 4 cards; a poll and a redraw fire; the 4 cards still stand and the count line is unchanged. The walk fails on main and passes with the fix.
- Owner-UI matrix: desktop and phone, light and dark, keyboard open and closed, with before and after screenshots and BUILD.txt.
- No regression in the existing search walks (server hits, typing fast, bucket chips, search scroll).

# Notes

- Reported through the coordinator (owner UI, fast-track). Slice 0 of the loading program #2426, which mu-clientperf owns.
- Related: #2200 search tab counts match cards (the fold), #1786 search scope server hits, #1229 keyed row patching, #1846 windowed rendering.
