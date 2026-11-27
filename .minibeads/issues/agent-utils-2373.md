---
title: 'reload-place-hot-load-drift: reload_place_browser lost the scroll anchor by 162 px on a busy host during a gate and never settled; not reproduced at load1 130'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
- owner-ui
created_at: 2026-11-27T06:48:00-05:00
updated_at: 2026-11-27T06:48:00-05:00
---

# Description

`vibe-talk.check.reload-place-browser` (#1849 vibe-talk-reload-keeps-view-and-place) failed in the owner gate on #2351 vibe-talk-read-strip-foot-reader-moves (head 48d31adf on main 1347494d, 2026-11-27 about 6:20 AM ET):

`phone: the anchor moved: {'id': '1000000000000000026', 'top': 90.625, ...} -> top -71.671875`

The anchor row ended 162 px up from where it stood before the reload. The whole check took 125 s: its poll for the kept place ran to the 120 s hang backstop, and the anchor never settled. Load1 around the step was about 170-198, with 2 gates running (from neighbouring step logs: the client-perf baseline at load1 198, the fold-latency loadavg 135/170/173).

The working hypothesis is a race on the reload and restore path, or in the check itself, under load. It is unproven: none of the local runs below were hot (load1 128-158, under the 237 that counts as hot). Two facts point away from #2351:

- The before-reload anchor stood at top 90.6 in the failing run. In every local run it stood at 76.9, so the page was already 13.7 px off before the reload.
- #2351's change only runs when the read strip's measured height changes. A ResizeObserver probe in a copy of the check logged exactly one strip report after the reload on both trees: height 0, hidden. So that code never ran.

Not reproduced at load1 128-158 (not hot), each tree from its own release build: base 1347494d (live) 13/13 green (5 one at a time, 8 at 4 in parallel), and 48d31adf 13/13 green, 4-6 s per run. Base never failed either, so no live regression is confirmed.

# Design

- Reproduce on base under a real hot load (load1 at or above 237, or while a full gate runs), recording load1 for every run. Hook `#scroll-area` scrollTop writes (a stack per write) and the session restore (when the kept anchor is read and applied), from the scroll before the reload to the end of the poll.
- Name the cause from that trace: which writer moved the list after the restore, or which state put the before-anchor at 90.6.
- Then, under owner rule 346 (gating checks are robust on a busy host): fix the check or the page code, whichever is wrong, with waits on events and conditions and no wall-clock budget. If neither can be made robust soon, disable the check from the gate with this issue tracking its return.

# Acceptance Criteria

- The cause named here from a trace, with a negative control that shows the failure on demand (for example a delayed restore or a late writer).
- The check green 10 times in a row with load1 at or above 237 (hot) for each whole run, load1 recorded per run.

# Notes

Filed at the coordinator's request (issue file only). The one exclusive owner rerun of the step on 48d31adf passed (4.2 s), and #2351 landed on it (39c0d887); the drift itself is still unexplained. Related: #1849, #2077, #2351, #2354.
