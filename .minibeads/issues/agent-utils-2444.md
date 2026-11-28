---
title: 'vibe-talk-overscroll-follow-short: the column overscroll check intermittently follows a reply a few px short of the right column''s foot, with the reader in the room'
status: closed
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- scroll-stability
- tests
created_at: 2026-11-28T01:55:00-05:00
updated_at: 2026-11-28T10:11:00-05:00
closed_at: 2026-11-28T10:11:00-05:00
---

# Description

`vibe-talk/tests/column_overscroll_browser.py --size desk-1920` fails intermittently on its FOLLOW step:

> FOLLOW: a reply arriving with the reader in the room was not followed as at the foot: scrolled 620 of 624, foot at 624, room 0px

The walk: the short thread in the right column, with the reader a little way into the room past its foot (`#338 column-overscroll`). A live reply arrives, and the reader must be followed as at the foot. In the failures the reader ends 4 px short.

Where it was seen (coordinator's attribution, 2026-11-28):
- on 7f578235 (#2437), a tree WITHOUT #2427;
- on #2423 trees built on dbecb0bb, which have #2427 but not #2437.
Neither change is implicated alone.

A controlled run found no miss (2026-11-28, `cl-flaketriage`). The check ran alone, alternating main 3700adec and 3c5da567 (#2427 and #2437 both live), each side running its own tree's test against its own page build, 10 runs each, at load1 191-428.
- main 3700adec: 10 of 10 passed.
- 3c5da567: 9 of 10 passed. The one failure was NOT the FOLLOW step: a negative control ("an arrival followed at the foot" page) failed at PAGING instead of ARRIVE.
- The FOLLOW miss appeared 0 times in 20 runs. At the 1-in-3 to 1-in-4 rate seen earlier, 0 in 10 on one tree would happen about 5% of the time.

So the miss is intermittent and not yet tied to any change. It may be a timing race in the room's fit around a live arrival.

# Design

- Reproduce first, where it was seen: the #2423 trees on dbecb0bb, and 7f578235. Trace every scroll write around the follow (the scroll owner's writes and the room's fit) at the moment the reply lands. Name the writer that leaves the reader 4 px short.
- Fix through the existing seams: the scroll owner and the room's fit. No new keeper.
- A change already parked on branch `cl-flaketriage/2444` (4b10b24b, not gated) keeps #2427's arrival-path measure (`followsNewest`, which reads the foot room) out of every column residency cannot act in, the right column included. It is hygiene, not a fix: the miss did not reproduce on 3c5da567. It is kept in case the trace points at that measure.

# Acceptance Criteria

- The FOLLOW miss is reproduced, with a trace naming the writer, on a tree where it was seen.
- After the fix, `column_overscroll_browser.py --size desk-1920` passes alone 10 times interleaved with main under the same load, and passes in a gate.
- No wall-clock budget in the check (owner rule 346).

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28, liaison 1135). Status open: intermittent, cause unknown.

Evidence: `cl-flaketriage` keeps the 20-run logs while this issue is open.

Related:
- #338: the right column's room past its foot.
- #2427: residency (live in 3c5da567; not implicated by the attribution above).
- #2437: the reply keyboard header fix (not implicated alone).
- #2423: the trees where it was seen with #2427.

## Resolution

[opus 5.5] Closed by `#2445` search-clear-two-columns-found-hold step 1 (90ff376b, live in 022cf893, deployed 2026-11-28 before 10:10 AM ET). Its trace named the cause.

The cause: on main, FOLLOW passed only by timing. The left column's Newest pill grows `#scroll-tools`, and `--scroll-tools-clearance`, shared with the channel's box, padded the right column's box too. A reply arriving with the reader in the right column's room left them short of the new foot. A frame later the pill hid and shrank the right foot under them, and the foot keeper's size-change follow closed the last pixels. When that frame slipped, the reader stayed 4 px short: this issue's miss. All of its recorded misses were at desk-1920, where the step is reached; at desk-1280 the walk does not reach FOLLOW.

The fix:
- The right column's box and reply pane keep no room for the left column's chips.
- FOLLOW now expects the row being read to keep its top (<= 0.5 px) with the reply shown below it (owner rule 370, `#2230`, liaison 1152), so no frame's timing decides the result.

Evidence (`column_overscroll_browser.py --size desk-1920`, fixture server, alone):
- Before the fix, the new FOLLOW against main's page failed 9 of 10 (the row moved 53 -> 57 px as the follow caught up); with step 1, 10 of 10 passed (load1 114-157).
- After the landing, on main 022cf893: 25 of 25 passed (2026-11-28 9:54-10:11 AM ET). 20 of those runs started under load1 237 (193-236), and the other 5 started at 237-256.
- The rebased head passed 3 of 3, with every negative control failing as it must; the owner gate's column-overscroll steps passed at 1280 and 1920.

The hygiene change parked on `cl-flaketriage/2444` (4b10b24b) is no longer needed for this step.
