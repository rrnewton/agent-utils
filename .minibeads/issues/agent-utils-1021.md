---
title: 'merge-queue-load-sensitive-tests: two load-sensitive tests count wall-clock instead of events'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-07T19:41:23.000000000+00:00
updated_at: 2026-11-09T02:53:16.000000000+00:00
---

# Description

Two merge-queue tests are load-sensitive in the same way
#1019 merge-queue-green-batch-overlap-race was: they assert on
wall-clock progress that CPU contention can starve, so they fail on a
loaded host while the behaviour they guard is correct.

- `cli_push_retry::a_head_stop_logs_at_once_and_detaches_the_in_flight_validation`:
  the run must log `stopped` and end without waiting out a slow
  (30 s) in-flight validation; it asserts the whole run takes under
  25 s. On a loaded host it was seen parked for about 121 s, which
  smells like a bounded 120 s wait somewhere in the detach path
  rather than the 30 s validator.
- `cli_stale_cancel::a_stale_cancelled_root_is_recomposed_on_the_new_base_not_retried`:
  passed 5/5 solo at 20 s each on tree 3b0b5026, but inside the
  gated full suite its donor waits starved (the merge-queue step
  took 577 s) and it failed. Its donor timing counts wall-clock.
- `cli_intake_window::an_ejected_change_resubmitted_at_a_new_head_is_validated_in_the_same_run`:
  a change resubmitted at a new head must be applied at the daemon's
  next intake point and land in the same run. In release-suite run 2
  on tree 3b0b5026 (2026-11-07) it failed with
  `assertion left == right failed, left: "queued", right: "landed"`
  at cli_intake_window.rs:389, then passed 5/5 solo runs (2-3 s
  each) on the same tree: the daemon's intake and validation
  scheduling starved under suite-wide CPU contention.
- `cli_donor::a_dead_donor_loses_its_lease_and_its_late_receipt_is_refused`:
  races `lease_secs=2` against `sleep(3)`: attempts come out 3 vs
  the expected 2 when the donor cannot admit within one lease under
  load. 1 failure in 2 full-suite runs on 2026-11-07; 5/5 isolated
  passes (~5.6 s each).

# Design

- For each test, find where its progress is bounded by the wall
  clock (a sleep, a fixed timeout, a daemon window counted in
  seconds) and make the oracle count from an event instead: the
  stop decision, the intake application, the validation verdict.
- Check whether the daemon window that decides "landed in the same
  run" should count from an event (intake applied, gate opened)
  rather than from daemon start.
- If a real product wait is found (for example the detach path
  waiting out a 120 s bound), name it here and split it out.

# Acceptance Criteria

- Both tests pass a contained 30-run sweep under load (16 nice-19
  CPU workers), as in #1017 merge-queue-flaky-probe-tests.
- Both tests still fail when the behaviour they guard is broken
  (the stop does not detach; the resubmission is not applied in the
  same run).
- The whole merge-queue crate suite is green on the landing tree.

# Notes

Follow-up from #1017 merge-queue-flaky-probe-tests (the two_reds
starvation, fixed and landed): these two were named there and in
the #1017 landing work but did not reproduce in its 30-run loop
(head_stop 0/30) or were found only once under full-suite load
(intake_window, run 2 above, explained by 5/5 solo passes).

The cli_stale_green prioritized test (landed under #1014) had the
same disease in a stronger form: its first wait read the seed run's
events and returned instantly, letting a submit win the writer lock
from the starting run and then timing out at 240s. Its fix counts
only events from the run under test, gates every ordering edge on
files, and widens depth so intake always has a window slot; see
#1014's Resolution for the full mechanism and mutation proof.

## Resolution

Landed on main as 3bd08c6ece73c6753d112b561dea4679a086a9bf (gated commit 6e63e234).

All four tests are event-oracled now; no product wait was found
(the head-stop test's ~121s park matched its own 120s harness wait
expiring under load: the only 120s bounds in the stop path are
unrelated donor/sync timeouts). The donor lease test raises
`lease_secs` to 600 after the ghost's stored 2s grant has expired,
so `attempts` counts claims rather than whether the live donor
could finish inside a fresh 2s lease. The stale-cancel recompose
test holds the slow root on a gate file and returns the third
root's job only after change 1 has landed, so the claim-time
sweep deterministically sees the moved base; it releases the slow
root at once, because the run decides flights in order and
recomposes only after that root decides (waiting for the
recomposed job first is a cycle that hung this test). The
head-stop test holds its slow validation on a gate file and
asserts the run exits before that validation records its own
completion (a marker file), instead of asserting a 30s sleep
beats a 25s wall bound; the marker also distinguishes a detached
exit from one that merely waited out the gate's self-expiry. The
intake-window test keeps its status/started-validation oracles
with 240s liveness bounds (~4x the worst daemon chain observed
under full-gate contention; ~3s solo). Two harness hardenings
came out of the sweep: donor job files the tests rewrite are now
written atomically (tmp+rename, as production writes them) after
the sweep caught a run stopping on a mid-truncate empty job
file, and donor thread scopes set their done flag from a drop
guard so a panic inside a scope cannot deadlock its join.

Acceptance: the contained 30-run sweep under 16 nice-19 CPU
workers passes 30/30 (15-23s per iteration at load 42-126) after
a first run failed at iteration 5 on the non-atomic job write
above. Each test still fails when its guarded behaviour is
broken, verified in a scratch worktree with the breaks reverted:
all stale-recompose layers disabled (stale-cancel fails), donor
report accepting an unleased receipt (dead-donor fails on the
refusal assertion), mid-run intake disabled (intake-window
fails), and a stop that does not cancel dropped flights
(head-stop fails at 125.9s on the completion marker). Gate:
selected gate on f3deb382 (validate --base HEAD~1, 33 steps) 32 passed / 1 failed with rust.merge-queue.test clean (591 passed / 0 failed, wall 535s) and the one failure a real clippy unused-mut in the head-stop test; after that one-word fix and a patch-identical rebase over the stranded-intake landing, the 4-step amendment rerun on the exact push tree 6e63e234 passed 6/6 (merge-queue 592 / 0 in 502s, clippy, format, client-names, embedded guides, launchers). The landed tree's four test files are byte-identical to the gated tree.
