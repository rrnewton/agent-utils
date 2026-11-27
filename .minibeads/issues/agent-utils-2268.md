---
title: 'vibe-talk-search-none-match-line-overflow: on desk the search None-match line runs past the message column edge'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T17:55:00-05:00
updated_at: 2026-11-27T04:05:00-05:00
closed_at: 2026-11-27T04:05:00-05:00
---

# Description

Found while reviewing the #2251 vibe-talk-search-current-view-scope evidence. On desktop 1280x800, in a thread view whose search matches nothing loaded, the None-match line ("Searched 2 messages in this browser, a complete history back to Nov 25, 02:14 PM (no gaps). None match.") is one long line from about x 208 to x 1071. The message column runs from about x 274 to x 1006, with its edge line at about x 1013, so the sentence crosses that edge, and it starts left of the column too. Light and dark. #2251 does not change the notice's markup or CSS, so the overflow comes from main's layout, not from that change. On a phone the scroll area is the column, so the line wraps and fits.

Cause: `#search-empty` (`.search-empty`, styles/voice/01-top-bar.css) sits at the top of the scroll area with `margin: 1.6rem 0.5rem`, outside the column's max-width. It takes the scroll area's whole width, not the column's.

# Design

- Lay the notice out in the column it speaks for: the same max-width and centring as the message list, so it wraps inside the column at every width. The `Search All — thread replies too` chip (`#search-in-all`) that stands beside it gets the same treatment.
- Keep what `.search-empty` is for (#129 message-search): centred, given room, the only thing on screen when it is up.
- With two columns up, the notice stays within the column whose search it reports.
- The notice coming or going must not move the row being read (#2230, #2256).

# Acceptance Criteria

- A browser check against the real page and a fake chat server: at 1280x800, 1920x1080 and 412x915, a None-match line long enough to wrap lies within the message column's left and right edges. Main is the negative control, red at 1280 and 1920.
- Screenshots before and after: desktop 1280x800 in a thread view and in All, light and dark, plus the standard matrix, each folder with its BUILD.txt.

# Notes

Owner-UI (not fast-track). Filed by cl-searchview at the coordinator's request, from the liaison's review of the #2251 evidence (desk shots of scene 127-search-scope-thread-elsewhere). Related: #2251 vibe-talk-search-current-view-scope, #129 message-search, #2269 vibe-talk-search-reach-sentence-beside-server-hits (the same line's last sentence).

## Resolution

[claude-opus-5-5, lane cl-pagenits] Landed in 341cb049 (2026-11-26 11:56 PM ET, liaison LAND NOW on an owner gate). The None-match line and the Search All chip (`#search-empty`, `#search-in-all`) stand in the reading column: `max-width: var(--reading-width)`, centred, with 0.5rem of side room, so on a desk the line wraps inside the message column instead of running the screen's width; the two-column layout and the side column's own line (`#side-search-empty`) keep their columns. tests/search_empty_column_browser.py (dag 185) measures the line inside the column at 1280, 1920 and 412 px in All and Main; screenshot scenes 129-search-none-match-all and 129-search-none-match-main show it.
