---
title: 'vibe-talk-all-view-search-both-buckets-default: in the All view, search starts with Main and Threads both on'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T14:12:00-05:00
updated_at: 2026-11-25T14:51:00-05:00
closed_at: 2026-11-25T14:51:00-05:00
---

# Description

Owner request, 2026-11-24 6:38 AM ET, in the thread that became #2200 vibe-talk-search-tab-counts-match-cards: "Note Main+Threads should both be turned ON by default when searching IN ALL VIEW. It should respect what we are searching."

All shows top-level messages and thread replies together, so a search started there searches both: the Main and Threads bucket chips start pressed, and their hits stand as cards. Main still starts with Main only, a thread view with this thread only, and the Transcripts screen with Transcripts only (the owner-146 table of #1797 vibe-talk-search-bucket-toggles).

# Design

- The default is data: the per-screen `defaultOn` of each bucket in the bucket table (`searchBucketDefs`, view/channel/olderwalks.ts). It is not a persisted setting and not in the URL: each new query (a new search identity) and each view change under the same query puts the buckets back to the defaults of the screen searched from; a chip the reader taps is their choice until the query or the view changes.
- In All: Main on, Threads on, Transcripts off. The bar shows it by the two pressed chips ("N in main", "N in threads") and by the count, whose first number is the sum of the pressed chips (#2200).
- Narrowing back: a tap on either chip turns its bucket off (and on again), as everywhere; Main is one view-picker choice away.
- Keyboard and phone: unchanged controls (the chips are buttons in the bucket row, reachable by Tab; the same row on a phone).
- Status: the default itself landed with #2200 (commit f7fa1ba7, on main in 8267bdfe, deployed 2026-11-25 1:46 PM ET), with the page test "in All, Main and Threads both start on and the fold still sums". This issue checks it end to end through the real server and records the evidence, and fixes any path where All does not start with both on (for example a Main-to-All switch under a standing query).

# Acceptance Criteria

- Page test: a search started in All has Main and Threads pressed and Transcripts unpressed; a search started in Main, then the view switched to All with the same query, ends with both pressed; switching back to Main ends with Main only.
- Dynamic check on a local test instance through the real server (never the live instance): in All, a word held by top-level messages and thread replies shows both chips pressed, cards from both, and a count equal to their sum.
- Screenshots: All with both chips pressed, phone 412x915 and desktop 1280x800, light and dark, keyboard open and closed, Discuss open in the phone shots, each folder with its BUILD.txt.

# Notes

Owner-UI fast-track (rule 286); issue number pre-assigned by the coordinator. Related: #2200 vibe-talk-search-tab-counts-match-cards (the fold and the default), #1797 vibe-talk-search-bucket-toggles (the defaults table), #1894 (extra buckets off by default), #2251 vibe-talk-search-current-view-scope (searching inside a thread or transcript narrows to it).

## Resolution

[opus 5.5] Already done by #2200 vibe-talk-search-tab-counts-match-cards: its commit f7fa1ba7 set the Threads bucket to start on in All, and it is live in 8267bdfe (deployed 2026-11-25 1:46 PM ET). No code change here; closed on evidence the liaison accepted. Page test on main: "in All, Main and Threads both start on and the fold still sums" passes; a throwaway probe also showed a Main to All switch under one query ending with both on, and All to a thread ending with only "this thread" on. Dynamic check on a local throwaway server built from main 00da5a15 (page build 812cad22), real routes only: a thread fixture seeded through the server's test-only fake-seed route, cached by the server's own timeline reads, then searched from All. Every frame shows "3 in main" and "2 in threads" both pressed, "0 in Transcripts" unpressed, and the count "5 found (2 loaded, 3 older)", with all five hits as normal cards: phone 412x915 and desktop 1280x800, light and dark, Discuss open, and keyboard open (viewport really shrunk, search field focused). Two pre-existing layout findings from those frames are filed as #2254 and #2255.
