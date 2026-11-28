---
title: 'vibe-talk-thread-open-layout-first: a thread opened on a slow connection shows a box for every reply from the first packets, filled in as bodies arrive, not one reply and then the rest at once'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T10:40:00-05:00
updated_at: 2026-11-27T14:30:00-05:00
---

# Description

The owner, 2026-11-27 10:06 AM ET, on a weak cellular connection, opened a thread with a root and 9 replies posted between 11 PM and 7 AM. For a long while the thread showed ONE reply (the 11:08 PM one). The rest then appeared all at once, so the owner concluded the posts had not gone through. Verbatim:

"The new work they're doing for incremental window loading should have at least populated the existence of these messages with their heights, so that blank boxes would have appeared from the very first packets that are received instead of waiting in this ambiguous state for messages to then all the sudden appear later."

Diagnosis (local only, page at main 699b9616, phone 412x915, API reads throttled; the device had seen the thread earlier with its root and one reply, eight replies were posted since, the page reopened in Main and the reader opened the thread):

- Opened after Main's read landed (800 ms round trip, 30 KB/s): the held reply showed at once with "refreshing…", no box and no loading line; the thread read took about 1 s, then all 9 replies landed in one draw. At 3000 ms / 8 KB/s: one reply for 3.9 s, then all 9.
- Opened right after the cached paint, before Main's read: the thread was drawn from the device's saved copy with no read. Main's read then brought the count of 9, but nothing re-read the open thread. It showed one reply until the next poll (52 s at a 30 s poll; a deployment polling every 120 s waits longer), then all 9 at once.
- No run ever drew a box for a missing reply, although the page knew the count. The root's exact reply count arrives with Main's read, and the page computes "reported but not held" (`threadOutrunsStore`, `storeLacksReplies` in `vibe-talk/web/src/view/channel/replycounts.ts`).
- Thread entry draws what the store holds and reads behind it quietly (`changeChannelView` in `vibe-talk/web/src/view/channel/channelviews.ts`; `readBehindSwitch` with `quiet: true`). A count that rises on an open thread schedules no read.
- The server answered this morning's thread reads from its copy in milliseconds. Before the bodies it knows each reply's id, time, author and length, but sends only the root summary's count.

# Design

The first slice of the sparse incremental loading design (owner requests 383/384, with the addendum for owner requests 386/387), scoped to the thread view. It is not a parallel design.

- A layout chunk read for one thread, answered from the server's copy only (held-answered in the state-sync matrix, any fill scheduled behind): total plus one entry per reply (id, time, author, length, flags, estimated), sent before the bodies.
- The page draws boxes at once from the count it already holds, then re-prices them one per reply when the layout lands, and fills each box in place with its row as bodies arrive. Boxes are real list items marked busy, never presentational spacers. An honest line counts the replies still loading.
- The same applies when a read raises an open thread's count above what is held: boxes at once, and the thread read at once rather than at the next poll.
- No Y-axis disruption (owner rule 370). Entry lands on the first unread as today; when that reply is still a box, its row takes the box's place at the same top. A fill above the anchor is one held correction.

Open owner choice: O1 (skeleton-first paint), scoped to the thread view. O2 to O4 are not needed by this slice.

# Acceptance Criteria

- Throttled walk through the real server router: on entry to a thread whose store lacks N replies, N boxes stand in the list before any of their bodies is folded; the layout read goes out before the bodies read; box heights equal the priced layout entries; every box becomes its row in place.
- The reader's row top moves at most 0.5 px on every frame of the fill, at the foot and mid-list.
- A count arriving for an open thread draws its boxes and reads the thread without waiting for the poll.
- Negative controls: a build without the layout read fails the height check; a build without the count trigger fails the open-thread case.
- A route test for the layout read, including its state-sync classification.
- Owner-UI evidence: phone and desk, light and dark, keyboard open and closed, Discuss open, including boxes mid-fill, plus a throttled before/after per-frame trace.

# Notes

Filed at the coordinator's request (owner-UI fast track), issue file only. The diagnosis and the slice plan went to the coordinator before any code.

## S-t2(a) landed (2026-11-27)

[opus 5.5, cl-signpost3] Landed as cabd6987 (gated head d0a13d89), deployed 1:42 PM ET, with a clean watch. What landed:
- the boxes from the count already held, with one "N more replies loading" line said once;
- the late-count read, so a thread opened from the saved copy is read when the count arrives, not at the poll;
- the rule-370 landing on the first box;
- a rollback kill switch, vibe-talk.voice.thread-reply-boxes "0", on by default and not shown in Settings (liaison: O1 confirmed for the thread view; on by default).

Throttled numbers at 3000 ms round trip and 8 KB/s:
- opened from the saved copy: boxes at +3.44 s, all 9 replies at +7.1 s (main: +52.2 s, no boxes);
- count already known: boxes in the first frame, all 9 at +3.9 s.

Landed under a one-landing waiver for discuss-browser: a known red with a new message, recorded in #2176's notes; 18 of 18 focused runs passed. Local post-deploy guards were green: thread_reply_boxes, thread-switch-fast, search_unmatched, two_column, and discuss_browser 3 of 3.

Still open: S-t1 (the server's layout read, giving each reply its id and size) with S-t2 (b, c) and a fixture that crosses the real router for threads. Related: #2391 (the coverage line shift, pre-existing) and #2393 (the label in view with the keyboard up).


## S-t1 landed (2026-11-27)

[opus 5.5, cl-loading] Landed as ae182bb0, 2f1318f0 and d474f915, a replay of gated d065dc8d. Not deployed yet: it waits for the next deploy range.
- **The layout read:** `GET /api/v1/channels/{id}/threads/{thread_id}/layout?limit=` (default 50) gives the total, its exactness, and one entry per reply: id, time, author, UTF-16 length, lines, flags (attachments, long) and `estimated: true`.
  - It reads the server's own copy only and starts no provider read. The state-sync tests classify it as Held.
  - An older server answers 404, and the page keeps the count's price.
- **The page:** it sends the layout read as the boxes first stand, before the bodies' read. It then re-prices each box from its own entry in one held change: the root, the held reply and the first box move 0.00 px on every frame.
- **Real router:** `tests/thread_layout_server_browser.py` runs the release binary on its fake upstream, through an opt-in `reply_count` on the fake's seeded roots.
  - The page's layout read reaches the real route, which answers 200 with all 9 entries.
  - The boxes match their prices to within 1 px, and the first reply stands where its box stood.
  - Main's page fails the walk.
- **The gate:** red only on read-pause-reply and search-unmatched, both off-path (12 of 12 solo runs green on main and on S-t1). Both landed under STEP-WAIVERs.
- **S-t2(b)** was already done by cabd6987. Nothing small is left of S-t2(c); #2391 and #2393 stay separate.
- **#2438**, the composer focus taking the reader to the foot, is pre-existing; priced boxes make the jump longer on a phone. It is filed separately and waits for the owner's pick.
