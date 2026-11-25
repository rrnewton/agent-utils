---
title: 'vibe-talk-search-reach-sentence-beside-server-hits: in All and Main the "does not reach yet" sentence stands beside the server hits'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T17:55:00-05:00
updated_at: 2026-11-25T17:55:00-05:00
---

# Description

Found while reviewing the #2251 vibe-talk-search-current-view-scope evidence. Take a server that holds every channel whole (`[message_cache] backfill = "full"`, every channel complete). In All or Main, search for words that no loaded message holds but the server's search finds. The count reads "1 found (0 loaded, 1 older)", the "1 in main" chip is pressed and the older hit stands as a card. Yet the None-match line still ends: "This server holds the complete history of this channel, which this search does not reach yet." The server's search did reach that history: its hit is on screen and its chip counts it.

Shown in the page model (the search fold fixture, Main and All, a word only the server's older copy holds). The sentence comes from #595 cache-full-history (2026-10-27), which predates the server's search standing in the search bar (#1762, #1786, #2200). It is true only of the browser's own search. #2251 already drops it in a thread view, whose search asks the server over its whole cache.

# Design

- Say the sentence only when it is true of what this search did. That means no server search answered these words (the server copy is closed, the ask failed or was refused, or the answer is still pending), or the server's answer stopped short of the whole history (more to fetch).
- When the server's search answered across the whole history, leave the sentence out. The chips and the count already say what it found; do not add a second sentence for it.
- The rest of the line ("Searched N messages in this browser ... None match.") speaks for the loaded messages and stays as it is.
- No layout change beyond the sentence itself; the line changing must not move the row being read (#2256).

# Acceptance Criteria

- Page tests in All and Main with the whole-history status: a server answer with hits and no more to fetch shows no reach sentence. The sentence stays when the server copy is closed, when the ask fails, and while an answer with more to fetch stands. The #595 test and the #2251 thread test keep passing.
- A dynamic check through the real router on a local test server (fake chat upstream, backfill full, history complete): in All, a word only older messages hold shows the chip and the card and no reach sentence. Main is the negative control.
- Screenshots: All with the server's hit standing, desktop 1280x800 and phone 412x915, light and dark, each folder with its BUILD.txt.

# Notes

Owner-UI (not fast-track). Filed by cl-searchview at the coordinator's request, from the liaison's review of the #2251 evidence. Related: #595 cache-full-history (the sentence), #2251 vibe-talk-search-current-view-scope (drops it in a thread view), #2200 vibe-talk-search-tab-counts-match-cards (the fold and its count), #2268 vibe-talk-search-none-match-line-overflow (the same line's layout).
