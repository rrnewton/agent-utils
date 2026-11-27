---
title: 'bridge-even-ring-snapshot-race: bridge_even_browser sees the phone keyboard ring rectangle in its wait, then reads it gone two frames later on a busy host'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- tests
- flaky-test
created_at: 2026-11-27T10:40:00-05:00
updated_at: 2026-11-27T10:40:00-05:00
---

# Description

`vibe-talk/tests/bridge_even_browser.py` (step `vibe-talk.check.bridge-even-browser`, `#524 thread-bridge-even-connectors` and `#526 pack-triangle-dim`) is a gating browser check. It reds now and then on heads that do not touch vibe-talk. It is not listed in `validation/known-red.toml`.

The failure on gate `dea51119` (`#2344`, a wrkslots test-only change; finished 2026-11-27 14:59Z):

    AssertionError: bridge-even-phone-393-light: the keyboard's ring draws no rectangle behind the triangle

The sequence in the touch walk (line numbers on main at `6dcbda58`):

- line 298 focuses the first unpacked row's `.unpack-pack` button with `focusVisible: true`;
- `desk.wait_for` (lines 299-304) passes only once that button's wash (its `::before`, or the button itself) has a non-transparent background;
- `desk.settled()` (line 305) waits two animation frames;
- `ROWS_JS` (line 306) reads the same first row's button the same way, and the check at line 308 finds the background transparent.

So the rectangle was drawn when the wait looked, and gone two frames later.

Sightings in the gate ledger (head, finished UTC, failed steps in that row):

- `a4b8a857`, 2026-11-23 00:06Z, 97 failed;
- `cbc7fd60`, 2026-11-23 13:31Z, 6 failed;
- `4a0f6bf7`, 2026-11-24 22:20Z, aborted, 24 failed;
- `7c346872`, 2026-11-25 01:29Z, 45 failed;
- `dea51119`, 2026-11-27 14:59Z, 7 failed (the only one whose log survives; the message above).

Rows that cannot be attributed:

- Earlier rows with the step red: `3713a713` 11-09, `b5b35619` 11-11 and `606d4720` 11-18. Their logs are pruned, so their failure messages are unknown.
- About 55 rows on 2026-11-20 with about 150 failed steps each. That was host-wide breakage, so they say nothing about this check.

The ledger does not record which steps each gate ran, so the failure rate is unknown.

Alone on `dea51119`'s tree, with that gate's own page build, the check passed 5 of 5 runs at load1 109-123 on 2026-11-27.

# Design

Investigation first; no code until the cause is named.

The hypothesis is UNPROVEN: the focused button loses its focus-visible state, or the row is re-rendered so the button that `ROWS_JS` reads is a new, unfocused node, between the wait and the snapshot. A candidate trigger is a side-card redraw from a live refresh landing under load.

To prove or refute it, on a failing run:

- record `document.activeElement` and the identity of the first row's button at the wait and again at the snapshot (for example, tag the node at the wait and look for the tag at the snapshot);
- record any side-card render that happens in between.

Try a deterministic reproduction that delays or forces the suspected redraw in that window, and keep load1 for each run (a hot run is load1 >= 237 throughout).

Any change to this keyboard-focus check, test or page, is the liaison's decision. That includes reading the snapshot inside the same wait condition, or re-focusing after a redraw. The rectangle is part of the owner's keyboard and touch rules (`#558`, `#526`).

# Acceptance Criteria

- The cause is named with evidence from a failing run or a deterministic reproduction, and the hypothesis is confirmed or replaced.
- A fix approved by the liaison: test-only if the page behaves correctly, or a page fix if the ring really vanishes for the reader.
- The check passes 20 consecutive runs alone and its gate step stays green on a hot host, with load1 recorded per run.
- No wall-clock budget is added (owner rule 346).

# Notes

Filed by `cl-flaketriage` at the coordinator's request on 2026-11-27, as an issue only. The red was found on `#2344`'s gate. The other failed steps in that row are tracked under `#2065`, the `search-typing-fast` known red, and `#2344` itself.
