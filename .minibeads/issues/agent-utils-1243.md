---
title: 'keyboard-bar-settle: the keyboard-hides-bar check races the send re-scroll under load'
status: open
priority: 2
issue_type: bug
assignee: mu-foldlat
created_at: 2026-11-10T18:33:14+00:00
updated_at: 2026-11-10T18:33:14+00:00
---

# Description

[opus 5.5] `vibe-talk.check.keyboard-hides-bar-browser` fails under gate load and passes alone. On 2026-11-10 it failed in three full gates across two slots, and passed in solo and fresh-worktree reruns of the same commits, including a bisect over three main commits.

The failure is `keyboard-bar-ios: after a send: the row's foot is 112px from the keyboard's top` (room 134 px), with the scroll position at 3276 against a furthest of 3404. The step already waits for the row (`wait_ios_row`) and then re-reads once (`ios_row_flush`). Under load, the send's append and re-scroll land between the wait passing and that single read, so the check measures a geometry that is still moving. This is a settle race, not a CPU budget problem, and it is separate from #1231 fold-latency-deflake (whose bracketed reference and majority re-walk do not touch it, and a majority re-walk here could mask a real geometry regression).

# Design

- Wait until stable at the measured geometry: after the sent row has drawn, require the asserted condition (row foot on the keyboard's top, scroll at its furthest) to hold across several consecutive animation-frame samples, bounded by a timeout, and judge only that stable state.
- On timeout, fail with the sampled geometry series in the message, so a real regression (the row never reaching the keyboard top) still fails and is diagnosable.
- Keep the assertion itself exactly as strict; only the moment of measurement changes.

# Acceptance Criteria

- Passes repeatedly under a representative gate load (for example 10 of 10), and passes solo.
- A deliberate geometry regression (the sent row left short of the keyboard top) still fails both solo and under load.
- The failure message shows the sampled geometry series.

# Notes

Found while working #1231 fold-latency-deflake (lead, 2026-11-10). Owner: mu-foldlat after #1231 lands.
