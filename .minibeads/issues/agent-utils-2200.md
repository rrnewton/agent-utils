---
title: 'vibe-talk-search-tab-counts-match-cards: search bucket counts match the cards shown, disjoint Main and Threads'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T06:36:00-05:00
updated_at: 2026-11-24T06:36:00-05:00
---

# Description

Owner 345/347 (2026-11-24): searching "theme" and "themes" gives
confusing, inconsistent results. The bucket chips say "2 in main"
(selected) and "12 in threads" (unselected), yet the view being looked
at contains more than 2 message cards — the count on the selected tab
does not equal the cards shown. Editing the query ("themes", then
deleting the "s") gives a different view again ("14 of 253 loaded"
beside a lone "2 in threads" chip with no Main or Transcripts chip).
Clicking "12 in threads" interleaves extra content rendered in a
different style, with no clear indication of what happened.

# Design

- The count on the selected tab always equals the cards shown: the
  tab state and the displayed result set come from ONE source (the
  same filtered list), never two computations.
- Main and Threads are DISJOINT: Main = top-level channel messages;
  Threads = thread replies only (non-main). The sum of the counts
  equals the total, and no card appears in both.
- Every status chip ("N of M loaded", Main, Threads, Transcripts) is
  a pure fold over the client's search-result state (the source of
  truth, synced from the server): an edited query gives the same
  state and chips as the fresh query, and the Main, Threads and
  Transcripts chips render consistently (never a lone chip).
- "theme" versus "themes": the matching rule (substring) is the same
  for the counts and the cards, on the client and the server.
- Clicking a bucket chip visibly indicates the selected tab, and its
  results render in the normal result style.

# Acceptance Criteria

- A test that the sum of the Main and Threads counts equals the
  total, and that no card appears in both.
- A test that an edited query ("themes", then backspace) gives the
  same state and chips as the fresh query "theme", plus a property
  test that the incremental update equals the from-scratch
  computation over random edit sequences.
- An end-to-end test through the real server route with the page's
  exact request, on a synthetic fixture with top-level and
  thread-reply messages containing "theme" and "themes".
- Dynamic testing on a local test instance, with the screenshot
  matrix (search open with each tab selected, counts visible, phone
  and desktop, light and dark) for the liaison before landing.

# Notes

Owner UI request (fast-track, rule 286); filed by the coordinator
(#2200 pre-assigned).
