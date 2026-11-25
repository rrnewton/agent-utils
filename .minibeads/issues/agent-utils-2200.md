---
title: 'vibe-talk-search-tab-counts-match-cards: search bucket counts match the cards shown, disjoint Main and Threads'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T06:36:00-05:00
updated_at: 2026-11-25T14:03:00-05:00
closed_at: 2026-11-25T14:03:00-05:00
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

## Resolution

[opus 5.5] Landed on main as 8267bdfe (land-main exit 0, 2026-11-25 1:42 PM ET; 25 commits, the landed tree identical to the gated 60beecf5), deployed 1:46 PM ET, deploy watch clean to 2:01 PM ET (0 errors, 0 restarts). Owner-tier gate green on 60beecf5 (base e21a1cd9; 1:18 to 1:35 PM ET, 225 steps, no failed step). The bucket chips, the count and the cards are now one fold over the page's search-result state: each message counts in its own bucket (a root in Main, its replies in Threads), so Main and Threads are disjoint and the chips sum to the count; the count reads found, loaded and older; an edited query lands on the same state and chips as the fresh query; the chips retire with the query that earned them while the list's head reservation holds, released in place only when the search settles with no chip. Tests: the page suite (the fold sums and selected counts equal the cards shown, Main and Threads disjoint, edited query equals fresh query over random edit sequences, thread view counts), the real-server search test with top-level and thread-reply messages holding "theme" and "themes", and the browser walks (search-typing-fast, search-unmatched, search-bucket-pinned, server-hits, transcripts); validation graph, screenshot self-test, typecheck. Screenshots: scenes 100 (the bucket fold) and 101 (Main only), phone and desk, dark and light. Liaison checks on 60beecf5 before landing: a fixed row does not move when the chips retire (back within 0.6 px once the answer lands; on main the same row moved 36 px, 102 px on desk), a search settled with no hits leaves no band under the search bar (phone, dark and light), the #2230 catch-up scene 106 keeps the reading row (dy 0, the jump says 1 new), and a 5-pair search-typing-fast spot check on a busy host passed 5/5 beside main's 5/5. Follow-up: #2249 (the "On this screen" signpost shows for one debounce window while the chips are down and moves the list 23 px).
