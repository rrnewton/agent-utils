---
title: 'held-dependent-after: holding the front change ignores a dependent --after, and accept-flaky cancel does not retire donor jobs'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-11T05:00:00.000000000+00:00
updated_at: 2026-11-11T05:00:00.000000000+00:00
---

# Description

With `validator.green_flaky = hold`, a GREEN_FLAKY batch is held for a ruling; accept-flaky lands the validated commit if the base has not moved. A change submitted with `--after N` must land after N; while N is ejected, withdrawn or not queued, the dependent is held ("never validated or landed without it").

Sequence (2026-11-11 UTC): #A (solo) and #B (--after A, stacked on A's candidate) validate on a remote donor; #C, an unrelated singleton root, validates on the local donor.

04:29:45Z: A comes back GREEN_FLAKY and is held. The queue discards B's stacked flight ("the head is held GREEN_FLAKY"), promotes C ("disjoint-root-promoted #C"), and re-plans B: "disjoint-root #B validates on the base itself, disjoint from #C", i.e. v4 on a candidate built on the base WITHOUT A, published as a donor job at 04:30:19Z. DEFECT 1a: B has --after A, and A is held, not landed, yet B is validated without A.

04:30:19Z accept-flaky A. 04:30:25Z the queue logs "accept-flaky cancelling #C, #B in flight: built on <base> without #A, which lands first", then lands A at 04:30:32Z.

DEFECT 1b: neither donor job was cancelled. C's local job finished 3 min later; at 04:33:20Z the queue accepted its red receipt and EJECTED C ("eject #C (failed-tests) ... on top of the base <old base>"), from a validation it had cancelled, on a stale base. B's v4 job was admitted by the remote donor at 04:30:25Z, the same second as the cancel. It is still running, status still shows B's batch active, the run cannot end, and B is at risk of the same false ejection (its tests expect A).

# Design

In `rs/merge-queue`:

- A held change's `--after` dependents stay held: never validated on the base without it. The disjoint-root re-plan after a GREEN_FLAKY hold must respect `--after` like any other hold.
- When accept-flaky (or anything else) cancels flights, it retires their donor jobs; a late receipt for a cancelled flight is discarded, never used to eject.

# Acceptance Criteria

- While A is held GREEN_FLAKY, B (--after A) is not validated on a candidate without A.
- After accept-flaky cancels flights, their donor jobs are retired; a late red receipt for a cancelled flight is discarded and ejects nothing.
- `cargo test -p merge-queue` (all targets) passes, including regression tests for both defects.

# Notes

Reported 2026-11-11 (queue at 2b55c42d). Defect 1a is the decision/re-plan path after a GREEN_FLAKY hold; defect 1b is donor-job retirement on flight cancellation plus late-receipt handling.
