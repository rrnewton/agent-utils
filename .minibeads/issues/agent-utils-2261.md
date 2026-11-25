---
title: 'vibe-talk-summary-link-back-race-diagnostics: dump the Back restoration state when the summary-link walk times out, and name the race'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- flake
created_at: 2026-11-25T16:12:00-05:00
updated_at: 2026-11-25T16:12:00-05:00
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
- The red run's step cgroup was throttled no more than green runs of the same step, so CPU pressure does not mark it.

The row stood away from its old top for the whole 20 s, so the restoration seems to land in the wrong place rather than land late. The logs of the first two failures are pruned, and the test prints nothing about the page's state when the wait times out, so the race cannot be named from any failure so far.

# Design

- On the timeout of that Back wait, before raising, have the test capture and print:
  - the main row's current top and the expected top;
  - the scroll owner's state (the list's scrollTop, any hold or pending restoration it reports, the panel and answer scroll positions);
  - `history.state` and `location.href`;
  - the walk label.
  Optionally also save a screenshot. Keep the existing 20 s condition wait; this adds evidence, not a timing budget.
- Apply the same dump to the walk's other position waits if they share a helper, so a failure anywhere in the Back path leaves evidence.
- When a captured failure shows the race, fix it in the page (or the test, if the test's expectation is wrong) and record which act moved the row.

# Acceptance Criteria

- On a timeout of the Back-restoration wait, the step's log shows the row top (current and expected), the scroll owner's state and `history.state`, with the walk label.
- The race is named from a captured failure (which act moved the row, or which restoration did not run), and either fixed with a regression check or tracked with that evidence.
- The step keeps waiting on conditions, not wall-clock budgets, and stays robust on a hot host.

# Notes

Filed from the #2232 owner gate (5d0695c9), where this step was the only red.
