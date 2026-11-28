---
title: 'vibe-talk-sparse-loading-program: one incremental loading strategy for the channel window, made the default and the only one, with islands and an O(1) Newest'
status: open
priority: 0
issue_type: feature
assignee: cl-loading
labels:
- vibe-talk
- owner-ui
- performance
- scroll
created_at: 2026-11-27T19:55:00-05:00
updated_at: 2026-11-27T19:55:00-05:00
---

# Description

The program that moves the channel window onto the sparse incremental loading design (owner requests 383, 384, 386 and 387; the approved design note in the harness repository, `plans/2026-11-26-vibe-talk-sparse-incremental-loading-design.md`). Only its first slice, thread placeholder boxes (#2385, S-t2(a)), is live.

Owner, 2026-11-27, verbatim:

- 7:15 PM ET: "Ok keep driving this without pause until it is implemented, tested, and shipped live for me to test."
- 7:24 PM ET (lag): "Pressing reply and popping open the reply window should be 100% LOCAL AND INSTANT... client<>server sync should be more background and asynchronous."
- 7:39 PM ET: "Well the old version of "Windowing" was completely broken when I turned it on so I turned it back off. we're looking for the new smart/incremental loading to become DEFAULT not maintaining multiple strategies. Also even on desktop I think 5000 messages is too large a target to load. The loading should be incremental and small. If we scroll back and back and back and load tons of messages then eventually it would break off into a "separate" island... keeping a contiguous region around our farback scroll, and keeping a small window of most recent, but losing what's inbetween. Jumping to "newest" should still be fast and constant time."

Where things stand on main (2026-11-27):

- Two loading strategies exist. The default holds a dense newest suffix (phone 300, desk 5000 messages as data) and mounts every held row; the windowed strategy (#1846, the `windowed-lists` device setting) mounts a render window over the same suffix. The windowed strategy is off by default and the owner found it broken when turned on.
- The owner's freeze reports show about 80 screens of mounted rows and hundreds of long freezes (#2427).
- A link to a message older than what is held walks at most five pages and then gives up in words.

# Design

ONE STRATEGY. The sparse design becomes the default and the only loading strategy for the channel window. The old windowed toggle and the load-and-trim path are retired, not kept beside it.

The residency model (owned by this program; other lanes build on its interface):

- Per channel and view, the page holds a small ordered set of islands: dense stretches it truly holds as data. Between islands are gaps it does not hold.
- The newest island is small: a bounded window of the most recent messages, not thousands.
- Scrolling back grows the island the reader stands in. When a reader scrolls far back, that island breaks off from the newest one: the page keeps a contiguous region around the far-back position plus the small newest window, and drops what lies between. The dropped stretch becomes a gap, drawn as height placeholders or a crease, never silently removed from the scroll height.
- Newest is O(1): it jumps to the resident newest window. It never re-fetches the middle and never walks history.
- Residents that must stay held while they are in use (pins): the row being read, a jump or link target, search hits while a search is open (#2422), the speaking row, the selection and the reply being composed.
- Only the reader's band plus a prefetch margin is mounted as DOM; everything else is arithmetic (heights in arrays, owner request 386).
- Eviction drops whole stretches farthest from every pin first, never a pinned one.

Slices, each its own gate, landing and deploy (no big-bang), each owner-UI fast track:

- (0) Search hits pinned while a search is open: #2422, owned by another lane; consumed here as the first pinned residents.
- (1) S-t1: per-reply heights for threads (the server's layout read for one thread: id, size, flags per reply, before the bodies), finishing #2385's open half.
- (2) A channel height vector from the server: a layout packet first (ids, heights, the collapsed bit) on the timeline answer and as its own chunk read, answered from the server's copy only.
- (3) Sparse residency: islands with skeleton boxes for countable gaps and a crease for uncountable ones, an anchor-held fill, a small newest window, the island break-off on deep scroll-back, and an O(1) Newest.
- (4) Message links always work: an instant box at the target, the target fetched first with one `around` read, context streaming outward, the target never moving (owner request 383).
- (5) Search hits and links as pinned residents of the same model.
- (6) Retire the old windowed toggle and the load-and-trim path; the setting keys stay readable only so a stored value cannot break the page.

The interim held-rows cap of #2427 is built as this program's residency layer, against an interface this program owns and reviews.

Owner choices (defaults as set by the liaison; the owner may veto any):

- O1: skeleton-first paint, yes, across the whole app.
- O2: sticky-bottom follow is a setting, default off (owner rule 370 as landed in #2230).
- O3: a first fill of about 10 messages each way around a target.
- O4: every timeline request carries what the page already holds, so the server answers only the difference.

Rules every slice keeps:

- No Y-axis disruption: the row being read moves at most 0.5 px on every frame of a fill (1.0 px where placement is on whole pixels), except for the reader's own act (owner rule 370).
- Every new server read answers from the server's own copy and schedules any fill behind the answer (the state-sync matrix classifies it).
- Reply open and other local acts never wait on the network (owner, 7:24 PM ET).
- Decisions live in pure modules with unit tests and no DOM; the renderer is a projection (owner request 387).

# Acceptance Criteria

- Exactly one loading strategy on main: no windowed setting, no load-and-trim path, no second code path selected by a device setting. A stored old setting value changes nothing.
- On a 5,000-message fixture channel through the real server router, phone 412 and desk 1280: the mounted row count depends on the viewport and caps only, and the held message count stays within the small residency budget at every step of a long scroll-back.
- A long scroll-back breaks off an island: the held set is the region around the reader plus the newest window, with a gap between them drawn at its height; Newest then lands on the newest window with zero timeline requests.
- A link to message 2,500 of 5,000 lands its row with exactly one timeline request, and its top moves at most 0.5 px across the whole fill.
- Countable gaps draw one box per message at its layout height; an uncountable gap draws a crease; no presentational spacer stands for unloaded messages.
- The rule-370 traces stay green with the new strategy: load, fill above, trim, arrivals, own sends, phone and desk.
- Counting checks only in gating (requests, rows, pixels moved, operation counts); no wall-clock budget (owner rule 346).
- Every UI-visible slice: owner-UI screenshot matrix (light and dark, keyboard open and closed, phone and desk, Discuss open), before and after, `BUILD.txt` naming the exact tree, reviewed before its owner gate.

# Notes

Filed by cl-loading at the coordinator's request (2026-11-27), issue file only. This lane takes the program over from the previous client performance lane, which becomes advisory.

Related: #1846 (windowed rendering, superseded as a separate strategy by this program), #2239 (windowed coverage shift), #2292 (windowed composer foot, closed), #2385 (thread placeholder boxes, slice S-t2(a) live), #2422 (search hits pinned, slice 0), #2427 (interim held-rows cap built as the residency layer), #2428 (reply open instant), #2429 (typing lag).
