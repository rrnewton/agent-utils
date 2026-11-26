---
title: 'vibe-talk-search-current-view-scope: searching inside a thread or transcript narrows to it, with a visible way back out'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T14:20:00-05:00
updated_at: 2026-11-25T15:10:00-05:00
---

# Description

Owner request, 2026-11-24 6:38 AM ET, in the thread that became #2200 vibe-talk-search-tab-counts-match-cards: "When we search inside a thread we should be narrowed to that thread view. If we search inside transcript view we should probably see only hits in that transcript. I.e. search the current view."

On main after #2200 the thread view already starts with only "this thread" on and counts only it, but: the bar never says what it searches (the field reads "Search loaded messages" on every screen); when the thread holds no hit and main or other threads do, the bucket row stays down ("0 of 2 loaded", "None match"), so there is no visible way to widen in place; and on the Transcripts screen the search bar turns the Transcripts bucket on but places its hits in the channel list, which that screen hides, while the view's own "Search transcripts" field is a second, local-only search.

# Design

- One rule, "search the current view": the scope is the view the bar was opened over, never stored (not a setting, not in the URL); it follows the view as the reader moves.
- Scope named in the bar: the field's placeholder and accessible name say what it searches ("Search this thread", "Search this transcript", "Search transcripts", "Search All", "Search Main"), and once typed the home chip says it ("N in this thread", "N in this transcript").
- Widening in place: in a scoped view the bucket row stands whenever any in-scope bucket has a result, so with no hit in the thread the reader sees "0 in this thread" pressed beside the offers ("N in main", "N in other threads"); a tap on an offer interleaves those hits, as today.
- Widening by leaving: the thread's Back ("Back to All"/"Back to Main") and the transcript's "All transcripts" keep the query and re-scope to the parent view with its defaults; the X and Escape close the search as today.
- Transcripts screen: the search bar is that screen's one search (the in-view "Search transcripts" field goes; its filtering moves under the bar's query). With a day open, the day shows only its matching chunks: held chunks that match plus this transcript's server hits not yet held, placed by time as normal chunk rows, marked; the count and the home chip "N in this transcript" agree. On the index, only the days whose label matches or that hold a hit are listed. Other transcripts' hits stand behind an "N in other transcripts" offer whose tap goes to the index with the query. Matching uses the bar's search language, as the channel and the server do.
- Keyboard and phone: the bar's chord and Escape work on every screen, the Transcripts screen included; the chips are buttons in the same row on a phone; the scope words fit the field at 360px.
- Landed in two steps, each with its own gate: (a) the scope words, the widen-in-place row and Back keeping the query; (b) the Transcripts screen.
- Step (a) found the thread view's offers undercounting: a thread view asked the server only below the thread's own oldest row and dropped every hit the page already held, so after reading All the offers said "0 in main" while main held matches. A thread view now asks from the newest (no `below`), keeps out only what stands on screen (the thread's own rows), and also offers the held messages outside the thread that match (the page's own state), each once; the search key carries the thread scope, so entering or leaving a thread asks once and moving between threads reuses the answer. The ask stays served from the server's own copy: no upstream read, ever (liaison guard; a real-router test counts the chat service's calls), and no ask per keystroke beyond the existing debounce. With an outside chip turned on, the count reads "N found (X in this thread, Y elsewhere)".

# Acceptance Criteria

- Page tests, phone and desktop sizes: the placeholder and accessible name per view; in a thread view with no hit in the thread and hits elsewhere, the row stands with "0 in this thread" pressed and the offers, and tapping one interleaves its hits; Back from a thread keeps the query and lands on the parent view's defaults.
- Page tests for the Transcripts screen: the bar narrows an open day to its matches (held and server-placed), the count equals the chunks shown, the index lists only matching days, and "All transcripts" keeps the query.
- An end-to-end test through the real server's search route with the page's exact request, on a fixture with transcript chunks in two transcripts and channel messages in main and a thread.
- Dynamic check on a local test instance through the real server, with screenshots of each scoped state (thread with hits, thread with hits only elsewhere, open transcript, transcripts index) plus the standard matrix: phone 412x915 and desktop 1280x800, light and dark, keyboard open and closed, Discuss open in the phone shots, each folder with its BUILD.txt.

# Notes

Owner-UI fast-track (rule 286); issue number pre-assigned by the coordinator. Related: #2250 vibe-talk-all-view-search-both-buckets-default, #2200 vibe-talk-search-tab-counts-match-cards, #1797 vibe-talk-search-bucket-toggles, #1608 agent-transcripts-timeline (the Transcripts view and its bucket), #2153 vibe-talk-transcript-view-reload-and-scroll, #2249 vibe-talk-search-signpost-hidden-while-reserved (the head reservation the widened row must not move).

## Resolution

Part (a) landed as 0e76dc9d (search names the view it searches; a thread view searches its thread with offers for main and the other threads; Back from a searched thread searches the view it lands on; a thread's None match makes no reach claim; scenes 126-128) on a green owner gate on the same tree (c7c40c2f, 2026-11-25 8:06 PM ET), after a first gate on the same change went red on cargo fmt and strict mypy in its test and scene code. Evidence: answer moment max dy 0.0 in all 8 traces; the cache-only server guard shows zero upstream calls with a negative control. Deployed the same evening; the 15-minute post-deploy watch was clean. Part (b), retiring the Search transcripts field, waits on the owner's decision and stays open here.
