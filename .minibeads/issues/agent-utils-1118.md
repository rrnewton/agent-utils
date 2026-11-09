---
title: 'browser-fixed-frame-settle: browser checks assert after a fixed two-frame wait that loses to paint timing under load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T09:34:09.000000000+00:00
updated_at: 2026-11-09T09:34:09.000000000+00:00
---

# Description

On 2026-11-09 the full gate on the #624 voice-page-modularity entering-a-thread
commit (55fa8bc4) failed vibe-talk.check.threads-polish-browser once:
"Fold read by h drew no run line in All: []" (read_mode_walk,
vibe-talk/tests/threads_polish_browser.py). The step had never failed in the gate
ledger before. It passed 5 of 5 solo reruns on the same commit, and the moved code
is not on that path.

The cause is the wait, not the page. The walk presses h, calls desk.settled(), and
then reads the drawn run lines. settled() (Desk in desktop_layout_browser.py, and
local copies in many other browser tests) is exactly two requestAnimationFrame
ticks with no condition. The run line is drawn by a render pass after the mode
change. Under full-gate load (about 150 steps with browsers in parallel), two frames
can pass before that pass paints, so the check reads an empty list while the mode,
label and status already say Fold read.

desk.settled() appears about 350 times across about 30 browser test files, and
several files define their own settled(). Other load-sensitive browser steps seen
in gates may share this pattern.

# Design

- Replace fixed-frame settles that precede a check with a bounded wait on the
  condition the check reads, using the existing Desk.wait_for(condition, why,
  seconds) or an equivalent page-side poll. For the failing walk: wait until the
  run line exists (bounded), then assert.
- Keep settled() only where nothing is asserted on the next render, or where a
  frame boundary is itself the point. Document that use at the helper.
- Sweep the browser harness for other fixed-frame or fixed-timeout waits that
  come before an assertion, and convert them the same way. List the files touched
  and any waits deliberately left alone, with the reason.
- Keep the bounds generous enough for a loaded gate host, and fail with the same
  messages as today.

# Acceptance Criteria

- read_mode_walk waits on the run line, not on two frames.
- The sweep's list of converted waits and deliberately kept settles is in Notes.
- The touched browser steps pass in a full gate and in solo reruns.

# Notes

Filed by the devbig030 coordinator at the lead's request, after the entering-a-thread
landing (0c621bd8). Gate evidence: the ledger entries for 55fa8bc4 (one gate fail,
five exclusive-rerun passes).
