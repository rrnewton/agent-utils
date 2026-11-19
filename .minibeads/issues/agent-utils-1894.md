---
title: 'vibe-talk-search-extra-hits-off-by-default: thread and server extra hits start off, toggled by their chip, and never move the scroll'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-19T00:34:25-05:00
updated_at: 2026-11-19T01:30:00-05:00
---

# Description

[watermelon-full-preview] Owner (thread 216; one environment name redacted): "I am doing a search for a string ("afl") in a channel now... and it looks ok for a second showing me filtered cards (I started at the bottom of the screen), but THEN the hits in threads populate and show by default AND jump me far back in the scrollback to the top. I would NEVER want this kind of extra search result to be on by default. I was able to collapse it... but I didn't want it at all. Before I asked for these "extra" results to be toggled WITH A CHIP under the search bar and to be off by default. And IF they are on they should appear inline in chronological order WITHOUT jumping the scrollback"

Trace (page build e53c7fee): `placeFilterAnchor` -> nudge wrote 836 -> -376 and 0 -> -297 at 05:10:07Z, and 6615 -> 6330 at 05:08:34Z, when the thread hits arrived.

Today the #1797 bucket chips under the search bar (`vibe-talk/web/src/view/channel/olderwalks.ts`, `searchBucketDefs` + `resetBuckets`) default per starting screen: in the All (flat) view the "in threads" bucket starts ON, and in a thread view the buckets that are not this thread start on in part. A new search therefore interleaves the extra buckets' server hits into the timeline unasked, and when that answer lands late, placing those rows nudges the scroll (`placeFilterAnchor` in `vibe-talk/web/src/view/channel/filterplace.ts`) far from where the reader started.

# Design

- Off by default, every search: at each new search identity, only the home bucket starts on (this thread in a thread view, main elsewhere, Agent Transcripts on the Transcripts screen); every extra bucket (threads/other threads, main from a thread view, transcripts elsewhere) starts OFF. The chips under the search bar remain the toggle, exactly as the owner asked; a bucket the reader turns on stays on for that search, and the next search starts from off again (`resetBuckets` already re-runs per identity).
- Inline without jumping: when an enabled bucket's hits are placed late, they interleave chronologically (existing `placeServerHits`) with zero scroll movement after the initial filter placement. The place keeper must not nudge after that first placement once the reader has scrolled: capture a `ScrollMark` when the hold is taken and skip the post-placement nudge when `ScrollKeeping.userMovedSince(mark)` says the reader moved (`#1880 vibe-talk-no-programmatic-scroll-after-user-scroll`, mu-readerdrill; reuse the exported predicate, do not track input a second time, and do not edit the same scrollkeep function uncoordinated).
- Manual proof only (owner clarification, thread 193): exercise the feature by hand on a real server (fake chat upstream) - search from the bottom, thread hits arrive late, scrollTop unchanged, chip off by default - and keep the evidence in the handoff. No new heavy DAG step.

# Acceptance Criteria

- A new search in All view shows the filtered loaded cards and no thread/server extra hits until the reader taps the "in threads" chip; the same for the extra buckets from a thread view and the Transcripts bucket off its screen.
- The chip row still names each extra bucket's count while it is off, and tapping it interleaves its hits chronologically.
- With an extra bucket on, late-arriving hits change no scroll position: from the bottom of the screen, scrollTop is unchanged when thread hits land, and the reader is never taken back to the top.
- Existing bucket, server-hits and place-keeping behaviour otherwise unchanged; the #1852 Whole-threads toggle and #1853 Labels filter compose as before.
- Manual real-server evidence (commands, scrollTop readings, screenshots with BUILD.txt) in the handoff; owner-priority gate green or waived steps named.

# Notes

Coordinates with #1880 (mu-readerdrill): their `ScrollKeeping.userMovedSince` export is on `mu-readerdrill/1880`, not yet on main at filing; the scroll half of this issue either follows its landing or cherry-picks it with their OK, never a second input tracker in scrollkeep.

## Resolution

[watermelon-full-preview] Both halves landed and the first deployed: default-off as 0bc8a697 (in 597ba683, deployed in the 7d94969e watch), zero-scroll as 253ff5ab + 747cd7a3. At each new search only the home bucket starts on (`searchBucketDefs` in olderwalks.ts); extra buckets stand only from their chip, and the next search resets them off. Late extra-bucket landings interleave through `placeServerHits` as before, but `placeFilterAnchor` no longer nudges the scroll once the reader has moved since the anchor's capture: the anchor carries the scroll owner's reader-input stamp (`ScrollKeeping.inputStamp`, #1880's guard f9bb641c), answered by `userMovedSince`, with no layout reads (the #249 same-search invariant holds). Gate evidence: default-off gate green except approval-card-small-phones (CPU timeout, untouched; exclusive rerun pass) and fmt NO_RESULT (OOM learned cap); zero-scroll gate green except discuss-browser and thread-pin-picker-browser (untouched timeouts; exclusive rerun pass for both). Manual evidence from main 7d94969e in /tmp/mu-labelsdrop-main with harness-format BUILD.txt, as reported to coordb.
