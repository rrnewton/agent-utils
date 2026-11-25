---
title: 'vibe-talk-summary-link-back-race-diagnostics: Back after a summary-link jump returns the row to its pre-jump top on phone (capture the race, then fix it)'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- flake
- owner-ui
created_at: 2026-11-25T16:12:00-05:00
updated_at: 2026-11-25T16:15:00-05:00
---

# Description

The gating step `vibe-talk.check.summary-link-browser` (`vibe-talk/tests/summary_link_browser.py`) went red on three unrelated heads in three days:
- 7500308c (#2133 page work, alongside two other steps);
- d2eaed93 (wrkslots-only, this step alone);
- 5d0695c9 (#2232, server-only WTF lookup work, this step alone).

On 5d0695c9 the phone-412-dark walk failed at `summary_link_browser.py:470`. After the reply-citation jump (a stay-jump in All), `page.go_back()` is followed by a `wait_for_function` that waits for the main-channel row's top to come back within 4 px of where it stood before the jump. That wait timed out after 20 s (`Page.wait_for_function: Timeout 20000ms exceeded`). The light walks on phone and desktop had passed in the same run.

The step does not reproduce on demand:
- On 5d0695c9: 5 of 5 sequential local runs pass (13-14 s each, host load about 100-120), and the gate's solo rerun passes.
- On its base: 3 of 3 runs pass.
- The red run's step cgroup was throttled no more than green runs of the same step, so CPU pressure alone does not mark it.

The row stood away from its old top for the whole 20 s, so Back seems to land the reader in the wrong place rather than land late. That is a reader-visible bug (Back after a summary link does not return the reader to the row they were on), not only a test flake.

The logs of the first two failures are pruned, and the test prints nothing about the page's state when the wait times out, so the race cannot be named from any failure so far.

# Design

Step 1, diagnostics:
- On the timeout of that Back wait, before raising, have the test capture and print:
  - the main row's current top and the expected top;
  - the scroll owner's state (the list's scrollTop, any hold or pending restoration it reports, the panel and answer scroll positions);
  - `history.state` and `location.href`;
  - the walk label.
  Optionally also save a screenshot.
- Record a per-frame trace of the Back landing: the row's top relative to its pre-jump top on each animation frame from `go_back()` until the view settles, printed on failure and checkable on success.
- Apply the same dump to the walk's other position waits if they share a helper.
- Try to provoke the race on demand: repeated solo runs, including runs inside a CPU-capped scope like the gate step's, so a failure is captured with its dump.

Step 2, fix:
- From the captured failure, name the act that moves the row or the restoration that does not run. Then fix the page's Back landing so the row returns to its pre-jump top on phone (and stays right on desktop), through the scroll owner (the scroll-writer guard stays enforcing).
- Turn the per-frame trace into the regression check.

# Acceptance Criteria

- On a timeout of the Back-restoration wait, the step's log shows the row top (current and expected), the scroll owner's state and `history.state`, with the walk label.
- The race is named from a captured failure (which act moved the row, or which restoration did not run).
- A per-frame trace of Back shows the row's top within 0.5 px of its pre-jump top on every frame once the restored view is drawn, at 412x915 and 1280x800, light and dark.
- The summary-link step passes 20 times in a row run solo on a hot host.
- The step keeps waiting on conditions, not wall-clock budgets.

# Notes

Filed from the #2232 owner gate (5d0695c9), where this step was the only red. Reframed the same day as a fix at owner tier (diagnostics first, then the Back landing fix).
