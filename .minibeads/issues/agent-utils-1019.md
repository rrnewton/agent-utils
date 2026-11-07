---
title: 'merge-queue-green-batch-overlap-race: green batches fail the no-overlap timing assertion under concurrent load'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T16:35:00.000000000+00:00
updated_at: 2026-11-07T16:35:00.000000000+00:00
---

# Description

`green_batches_land_in_order_with_speculation` in
`rs/merge-queue/tests/cli_run.rs` intermittently fails its assertion
that speculative green batch validations overlap:

```
thread 'green_batches_land_in_order_with_speculation' panicked at merge-queue/tests/cli_run.rs:658:5:
no overlap: 1015-1616 vs 1642-2243
```

The exact ranges vary; another sweep run reported
`no overlap: 760-1349 vs 1520-2122`. The assertion compares two timed
validation intervals and requires their timestamps to overlap. Under
concurrent CPU contention, the second validation can start and finish
after the first interval despite being launched as speculative work,
so the test fails even though all batches land in order.

# Design

- Determine whether the product ever fails to launch the two green
  validations concurrently, or whether the test's wall-clock overlap
  assertion is the only race.
- If the product is correct, make the oracle observe launched
  concurrency directly (for example, ordered start records whose
  overlap is independent of completion scheduling), rather than
  requiring two wall-clock completion intervals to cross.
- If the product can serialize the launches, fix the launch path and
  name the mechanism in the issue resolution.
- Keep the test's in-order landing and speculation coverage intact.

# Acceptance Criteria

- A contained 30-run sweep of
  `green_batches_land_in_order_with_speculation` has zero failures.
- The strengthened test fails if the two speculative validations are
  deliberately serialized.
- The whole merge-queue crate suite remains green.

# Notes

Reproduced on 7 November 2026 on tree a39f8984 with the
parallel-experiment runner: 30 nice'd workers at width 4 produced 13
hits (seeds 0, 2, 3, 4, 5, 6, 9, 10, 13, 19, 22, 26 and 29). This is
separate from #1017 merge-queue-flaky-probe-tests. Do not use a
single passing run as evidence; host-load timing can hide the race.
