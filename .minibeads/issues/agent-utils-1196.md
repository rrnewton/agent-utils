---
title: 'front-starve: an unstarted front starves while donors run newer disjoint roots'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-10T07:30:00.000000000+00:00
updated_at: 2026-11-10T07:30:00.000000000+00:00
---

# Description

Run 20261110T074129Z (started 2:41:29 AM ET, base b78d8c99) planned six flights in order: the front v1, then v2, v3, v4, and two disjoint roots v5 and v6 built on the base itself. The local donor's cap rose from 0 to 1 at 2:46:58 AM and to 2 at 2:47:43 AM. At 2:47:04 AM the donor leased v5, not the front v1; its next claim was v6, the newest job. v1 to v4 stayed queued with zero attempts. v5 went green at 3:08:49 AM (1304.1 s) and v6 at 3:09:09 AM (1283.0 s), but neither could be decided: `next_decidable_index` returns none at the first flight with no result unless it is a stale disjoint root, so both greens waited on the unstarted front. Nothing landed until an operator withdrew the four never-claimed changes at 3:14:47 AM; one root then landed, the other's green was thrown away (rebuilt after the base moved), and the withdrawn changes were resubmitted behind it. Cost: 21.4 minutes of donor validation discarded plus a re-validation, 29 to 33 minutes of queue delay for the front flights, and operator time at night.

# Design

In `rs/merge-queue`:

- Donor claim order: when a donor claims, it takes the deciding job of the earliest undecided flight in plan order, not the newest job. Find why the 2:47:04 AM claim took v5 then v6 while v1 was claimable (newest-first ordering, a tie-break, or the tool re-stamp at 2:53:39 AM) and fix the order.
- Decision order: an unstarted front (never claimed, no validation spent) no longer blocks a green disjoint root on the current base. `next_decidable_index` may decide such a root ahead of the unstarted front; landing the root first is safe because the front just rebuilds on the new base. Or re-plan so an unstarted front never blocks greens behind it.
- Regression test: a front that is never claimed, with two green disjoint roots behind it. One root lands with no operator action, and no green is thrown away for a front that never started.

# Acceptance Criteria

- Donor claims follow plan order: the earliest undecided flight's deciding job is claimed before any newer job.
- With an unstarted front and two green disjoint roots behind it, one root lands with no operator action and no green is discarded for the unstarted front.
- `cargo test -p merge-queue` (all targets) passes, including the regression test.

# Notes

Full report: the 2026-11-10 front-starve report relayed to this lane (code read at c42f0c71; `next_decidable_index` near `rs/merge-queue/src/run.rs:5233-5320`). If the decision-order change conflicts with the singleton-ejection decision order, stack it on that change.

Second data point (2026-11-10, 4:04 to 4:32 AM ET), on the serve's requeue-at-front path in the consumer's queue: six disjoint roots in serve plan order, every deciding job queued with zero attempts, yet position 4 was claimed before position 1. From the job records: position 4's job carried a plan index matched to the run's first plan (a recomposed flight), while position 1 (an intake flight) and other recomposed flights carried none, so indexed jobs sorted ahead of every unindexed one regardless of age. The green at position 4 then could not be decided behind unclaimed flights ahead, and was only landed after manual withdrawal of two changes ahead of it. Coverage: claim order now compares plan indices only between jobs that both carry one (mixed pairs fall back to age), beside the prefix-flight index at publish.
