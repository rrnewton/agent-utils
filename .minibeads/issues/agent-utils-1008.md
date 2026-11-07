---
title: 'donor-claim-plan-order: Store::claim leases the oldest queued job by id, ignoring plan order and prioritized changes'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-07T07:10:00.000000000+00:00
updated_at: 2026-11-07T07:10:00.000000000+00:00
---

# Description

`Store::claim` in rs/merge-queue/src/donor.rs leases the oldest queued job by job id. It ignores the queue's plan order and whether a change was prioritized (`submit --prioritized`, or the priority label). A prioritized change published last waits behind every job already published; with two deciding donor slots that costs 15-20 minutes per prioritized change on a consuming project's queue (measured by the consumer's queue operator, 2026-11-07).

# Design

- Record claim order on the job at publish time: whether any of its changes is prioritized, and its batch's index in the queue's plan.
- `Store::claim` leases by that order first (prioritized ahead of unprioritized, then plan index), with job age (job id) as the tiebreak.
- Backward compatible: jobs published before this field exists carry neither value and sort by job id among themselves, as today.
- Local and remote donors claim through the same `Store::claim`, so both follow the same order.

# Acceptance Criteria

- A prioritized job published last is claimed first.
- Equal priority keeps oldest-first.
- Jobs without the recorded order (old job files) are claimed as today.
