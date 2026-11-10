---
title: 'keyboard-bar-settle: the keyboard-hides-bar check races the send re-scroll under load'
status: closed
priority: 2
issue_type: bug
assignee: mu-foldlat
created_at: 2026-11-10T18:33:14+00:00
updated_at: 2026-11-10T19:25:00+00:00
closed_at: 2026-11-10T19:25:00+00:00
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

## Resolution

[opus 5] The after-send measurement in `vibe-talk/tests/keyboard_hides_bar_browser.py` now waits
until the row's geometry is stable before judging it: `wait_ios_row_stable` samples the row's foot
gap, the list's scroll and its furthest on every animation frame and settles only when the asserted
condition (gap within the existing slack, scroll at the furthest, the list's height unmoving) holds
across five consecutive frames, bounded at 15 s. The assertion itself (`ios_row_flush`) is
unchanged and still judges a fresh single read taken right after the settle. On timeout the failure
message prints the sampled series (`gap … scroll …/…` per frame), which a probe against unsettled
geometry confirmed. Evidence on 2026-11-10: the updated check passed solo, then 10 of 10 with one
gate admitted beside it at loadavg 103-137; a variant page whose keyboard room is 112 px too tall
(the row left 129 px short of the keyboard's top, room 246 px) failed both solo and under that
load. The step stays a shared DAG step; only the moment of measurement changed. The variant was
built only under /tmp from copied sources; no page code changed.
