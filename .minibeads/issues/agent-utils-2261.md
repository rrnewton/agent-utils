---
title: 'vibe-talk-summary-link-back-race: Back after a summary-link jump returns the row to its pre-jump top on phone (capture the race, then fix it)'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- flake
- owner-ui
created_at: 2026-11-25T16:12:00-05:00
updated_at: 2026-11-25T18:48:00-05:00
closed_at: 2026-11-25T18:48:00-05:00
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
- From the captured failure, name the act that moves the row or the restoration that does not run. Then fix the page's Back landing so the row returns within 0.5 px of its pre-jump top on phone (and stays right on desktop), through the scroll owner (the scroll-writer guard stays enforcing).
- Turn the per-frame trace into the regression check.

# Acceptance Criteria

- On a timeout of the Back-restoration wait, the step's log shows the row top (current and expected), the scroll owner's state and `history.state`, with the walk label.
- The race is named from a captured failure (which act moved the row, or which restoration did not run).
- A per-frame trace of Back shows the row's top within 0.5 px of its pre-jump top on every frame once the restored view is drawn, at 412x915 and 1280x800, light and dark.
- The summary-link step passes 20 times in a row run solo on a hot host.
- The step keeps waiting on conditions, not wall-clock budgets.

# Notes

Filed from the #2232 owner gate (5d0695c9), where this step was the only red. Reframed the same day as a fix at owner tier (diagnostics first, then the Back landing fix) and retitled from vibe-talk-summary-link-back-race-diagnostics to vibe-talk-summary-link-back-race. A fresh lane takes it.

## Captured (2026-11-25)

The step-1 trace (7e4f4090) caught the race on the 27th plain solo run of the step (desk 1280, light, host load near 99); the same shape fits the phone red on 5d0695c9:
- 12 ms: the reply citation lands (`landOnDeepLink` -> `revealRow`), writing the list from 1074 to 39 and queueing the landing's per-frame re-assert (#1820, #2147) for the next frame.
- 23-26 ms, the same frame: Back (`returnFromLinkJump` -> `putBackOnSpot` -> `restoreScroll`) puts the main row back, 0.19 px from its pre-jump top.
- 29 ms, the next frame: the reply landing's re-assert runs its first frame. Nothing on Back ended that landing, so it still counts as current; it finds its row off centre and nudges the list back to 39, undoing Back.
- With the list at 39, under the older-page trigger (80 px), an older page loads and holds the top row, so the main row stays 1074 px away for good.

So Back did not end the landing it undid. The window runs from a landing until its first re-assert frame, and stays open for up to 30 frames while the layout still moves (for example the phone sheet settling). It is wider on a loaded host. The bug is in the page, not the tree, so it showed up on unrelated heads.

## Resolution

Step 1 (diagnostics) landed as ec77a953: on a timeout of the Back wait the test dumps the row top, scroll state, history.state and a per-frame Back trace with every list write and its stack. The captured failure named the race: the citation landing queues a per-frame re-assert for the next frame; when Back is handled in that same frame, Back restores the row and the landing's re-assert then scrolls back to the reply, undoing Back (an older-page load then holds the wrong spot). The fix landed as 2277a9e3 (Back ends the landing it undoes and drops the jump anchor on the row left) on a green owner gate on the same tree (08517651, 2026-11-25 6:28 PM ET). Evidence: a race-window probe that forces Back into the landing's frame fails every hit on main (4 of 4; 1074 px desk, 1389 px phone) and holds every hit on the fix (14 of 14); the committed step passed 20/20 solo with a per-frame Back check within 0.5 px (max drift 0.19 px); a fake-rAF module test and a navigation test fail with the fix removed. Load note: the 20 solo passes ran at load1 92-126 (29-40% of 316 threads), NOT at the >= 75% hot level this issue's acceptance named; a 20x rerun at load1 >= 237 is a follow-up. Deployed the same evening; the 15-minute post-deploy watch was clean. Follow-up: #2270 (the Linked message bubble stays on the reply row after Back).
