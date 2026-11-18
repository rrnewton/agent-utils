---
title: 'vibe-talk-search-history-up-arrow: Up and Down in the search box cycle previous search terms'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- search
created_at: 2026-11-17T20:00:18-05:00
updated_at: 2026-11-17T20:00:18-05:00
---

# Description

[watermelon-full-preview] Owner, 2026-11-17 (owner thread 155): "TODO: really want up arrow in search window / I thought I asked for this before.. when I Cmd-/ to pop search I want to press up and have my previous search terms there."

So in the search box (opened by Cmd-/ or Ctrl-/ or the search glass), Up and Down cycle through previous search terms, newest first, like shell history.

Prior-ask report (the owner asked what happened to the earlier ask): an exhaustive search of `.minibeads/issues/` (all 1515 files, bodies included), `.minibeads/comments/`, `ai_docs/`, `vibe-talk/README.md` and `git log --all` for "search history", "up arrow" near search, "previous search", "recall" and "recent searches" finds NO earlier issue, comment, plan or commit asking for search-term history. The nearest neighbours are `#344 search-shortcut` (Cmd+/ opens the search from anywhere, landed 2026-10-18 — the chord this feature builds on), `#1167 jump-to-unread-chip` (an on-screen up-arrow chip, unrelated) and `#526 pack-triangle-dim` (collapse triangles, unrelated). The earlier ask was therefore most likely made in conversation and never filed as an issue; nothing was dropped or reverted — it was never tracked. This issue is its first tracked form.

# Design

- A small owner in `web/src/core/search/history.ts` (`createSearchHistory`): the terms, newest first, bounded at 50 (`SEARCH_HISTORY_LIMIT`), de-duplicated (a re-run term moves to the front), kept on the device in local storage under `vibe-talk.search-history`; storage absent or refusing leaves the page working with an in-memory list only.
- A term is remembered when a search runs: Enter in the field (the gesture that already marks the hits and searches further back). Blank terms are never remembered.
- Navigation state lives with the list: Up from the field's current text remembers that text as the draft and recalls the newest term; further Ups walk older, stopping at the oldest. Down walks newer, and Down past the newest returns the draft exactly as typed. Typing (any `input` event) ends navigation, so the edited text becomes the next draft. Recalling applies the term as if typed: the filter, the marks and the count follow at once.
- Wiring stays in `web/src/view/channel/filterchips.ts`'s field keydown listener, beside Enter and Escape; Escape still closes and clears as today, and closing ends navigation. Arrow keys are swallowed only when a term is actually recalled or the draft restored, so the caret keys keep their ordinary meaning otherwise. The field's title/aria wording names the new keys.

# Acceptance Criteria

- Up at an empty box recalls the most recent term; repeated Up walks older terms newest-first and stops at the oldest; Down walks back newer and, past the newest, restores the text that was in the box when Up was first pressed (the draft), character for character.
- Typing then Up keeps the typed draft: Down returns to it. Editing a recalled term ends navigation; the next Up starts again from the newest term with the edited text as the new draft.
- Enter remembers the term; running the same term again moves it to the front without duplicating it; blank/whitespace terms are never stored; at most 50 terms are kept, the oldest dropped first.
- Terms survive a reload (local storage); a corrupt or non-list stored value is ignored, not fatal.
- Escape closes and clears exactly as before; ArrowUp/ArrowDown with no history (or Down while not navigating) do nothing and are not swallowed.
- Core unit tests for the list and navigation, and page tests driving real key presses in the field for each clause above.
- Screenshot matrix per the standing acceptance (light and dark, keyboard open emulated and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus before/after of the recalled-term state), every image looked at, folder path and one line per image in the report.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as 238bcec338147b5d84098787036ef17ba89ff920 (replay of gated ae45e4e8, verified combined tree da633984). `createSearchHistory` in `web/src/core/search/history.ts` holds the last 50 run terms newest-first, de-duplicated, on the device under `vibe-talk.search-history`; Enter remembers, Up/Down walk it from `filterchips.ts` with the typed draft restored past the newest, typing ends the walk, Escape unchanged. Core, page and view suites cover every clause; the full local validate passed, and the gate's only red (read-aloud-blocks geometry) passed 3/3 solo on this tree and on main and again on the exclusive rerun. Screenshot matrix in /tmp/mu-searchhist-shots1816 (32 images, inspected; Discuss open in the phone shots).
