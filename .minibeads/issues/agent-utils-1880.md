---
title: 'vibe-talk-no-programmatic-scroll-after-user-scroll: no programmatic scroll moves a view the user has scrolled'
status: closed
priority: 0
issue_type: bug
assignee: mu-readerdrill
labels:
- vibe-talk
- ui
created_at: 2026-11-18T17:50:00-05:00
updated_at: 2026-11-18T22:05:00-05:00
closed_at: 2026-11-18T22:05:00-05:00
---

# Description

[watermelon-full-preview] Owner report, urgent (owner threads 202/204, 2026-11-18), verbatim: "In this very thread as I try to read your messages I am STILL getting random scroll jumps upward... do you see it in my scroll log?" and "I had one just now while I wasn't pressing anything and I don't think even moving the mouse... i was just reading and it jumped". And (threads 206-207): "Huh it may also be at some time delay AFTER I send... when I am then reading." / "Indeed after that send I counted about 5 seconds in my head and then it jumped."

Trace (01:44:33Z, thread view): upward programmatic writes with no user input — `settleAfterRead -> toPosition` 4x pulling up 150-180 px; `placeDrillAnchor -> nudge` to -669 from `payThreadEntry` and the drill keeper's ResizeObserver correction 4x (the anchor re-applies long after entry, the #1856 root in another holder); `keepComposerInView` -74 px. Hypothesis: live replies arriving in the open thread re-render and resize it, firing the settle restore and the anchor correction. The send case: the optimistic local copy is replaced by its live echo about 5 s later, changing the row height while the post-send mark-read settle and the anchor correction fire.

# Design

One rule, in the existing holders, no new writer: once the user has scrolled or touched in a view, no programmatic write moves it, except a user action (a jump, a chip, a send, an explicit drill).

- The scroll-keeping owner tracks the reader's own input (wheel, touch, and scrolls it did not write). A capture (`captureScroll`) remembers the input time; `restoreScroll`, the redraw hold in `holdingRows`, and `keepComposerInView` do not write when the reader has given input since the capture.
- The drill anchor stops correcting after the entry settles: `payThreadEntry` places it once, the keeper holds it through the entry's late layout, and the next read landing after placement settles the entry and stops the keeper. Later reads (live arrivals, mark-read settles, the post-send echo) never re-place it. The post-send scroll to the sent message is a user action: it fires once, at send, and the echo does not re-fire it.

# Acceptance Criteria

- Unit proof in the channel JS suite: a capture followed by reader input is not restored over, for both the anchor restore and the composer hold; a capture with no input since is restored as before.
- A manual dynamic verification on a real server with the scroll trace: in a thread, scroll up mid-thread, a reply arrives live, mark-read runs, the panel resizes, and (the send case) send, scroll up, the echo lands ~5 s later — scrollTop does not move in either, and the post-send scroll fires once. Evidence in the handoff with shots and BUILD.txt.
- Owner-priority gate.

# Notes

Owner-priority, lands ahead of #1872. Lane mu-readerdrill on branch mu-readerdrill/1880, worktree readerhold. Related: #1818, #1839, #1856.

## Resolution

[watermelon-full-preview] Landed on github/main as f9bb641c55faf801d6785b40bd0a5a778c8832e9 (gated head b561a035, owner gate validate: OK; verified-tree route, replay tree ead7ca23 verified with typecheck, channel JS and composer-stays). The scroll owner tracks the reader's own input; captures carry it (`ScrollMark.inputAt`, exported `userMovedSince` for other holders, #1894), and restores/holds do not write after it (a focused composer that stood in view is still kept, #1839). The drill anchor is placed once; the next read landing settles the entry and stops its keeper. Trace evidence: a user-scrolled thread held at delta 0 through a live arrival plus growth on the fixed tree, against 627 -> 478 (-149px) on unmodified main; composer-stays passes at both sizes again.
