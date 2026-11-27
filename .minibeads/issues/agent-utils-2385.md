---
title: 'vibe-talk-thread-open-layout-first: a thread opened on a slow connection shows a box for every reply from the first packets, filled in as bodies arrive, not one reply and then the rest at once'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T10:40:00-05:00
updated_at: 2026-11-27T10:40:00-05:00
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
