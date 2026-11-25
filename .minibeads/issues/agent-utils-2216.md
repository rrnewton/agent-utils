---
title: 'vibe-talk-pace-availability-flake: Pace popover tap races page render under load'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-24T09:45:00-05:00
updated_at: 2026-11-24T20:10:27-05:00
closed_at: 2026-11-25T01:10:27.000000000+00:00
---

# Description

`vibe-talk.check.pace-availability-browser` fails intermittently with
"pace-enabled-light-1280: Pace did not open its popover". The ledger
shows the step red on six different heads since 2026-11-23 (among them
heads whose changes do not touch Pace, voice, or the read-aloud bar), and
five solo runs of the unchanged step on one of those heads pass 5/5.
The walk signs in, presses Read, reads the Pace tile's bounding box,
taps its centre once by coordinates, then waits up to 30 s for
`#speed-popover` to unhide. Under gate-level CPU contention the tap can
land before the bar's tap handler is live or after a re-render has moved
the tile, so no tap ever reaches the control and the wait expires; run
alone, the same walk opens the popover in seconds.

# Design

- Make the tap race-free: re-read the tile's bounding box and tap again
  until the popover's own condition (`#speed-popover` unhidden) holds,
  bounded by the shared hang backstop (120 s), instead of one tap plus a
  fixed 30 s wait. Alternatively wait for a handler-ready condition on
  the tile before the single tap; the retry-tap loop is the smaller
  change and matches how the glossary scenes re-drag until their chip
  stands.
- Keep every existing assertion (tile muted state, title, aria, popover
  contents) unchanged; this is a walk-mechanics fix only.
- No wall-clock budget as the success criterion: the loop's exit is the
  popover condition itself.

# Acceptance Criteria

- The step passes 5/5 solo on a loaded host and inside a full gate.
- A deliberately stalled popover (handler attached late) still passes,
  proving the wait follows the popover's own event, not timing.

# Notes

Found while gating #2189 vibe-talk-wtf-lookup-miss-diagnosis; the red
step is unrelated to that change. Filed by the mu-wtfdiag lane.

Closed: landed on main as 5703c2e8 (98e1a485 + 5703c2e8). An early-tap
recorder stands from the bundle's first moments and remembers the first
tap on the Read and Pace tiles; the mounts replay it through the
now-wired control and mark the tile `data-wired`, the readiness
condition the walk waits on. The walk taps at the earliest drawable
moment and re-taps until the popover's own condition holds. Full owner
gate on b806dd8e passed 2026-11-24 8:08 PM ET.
