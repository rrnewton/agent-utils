---
title: 'merge-queue-positional-validation-order: assert probe identity, not position'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-13T01:26:29.968500317+00:00
updated_at: 2026-11-13T01:27:18.371206718+00:00
closed_at: 2026-11-13T01:27:18.371206608+00:00
---

# Description

A merge-queue CLI test asserts which culprit probe ran by position: validations[1] must be the whole-candidate confirming rerun because it shares the batch's commit. Probe flights run concurrently and are recorded as they finish, so under load the base probe can finish first and the assertion reads the wrong record (observed in a gated run: position 1 held the base commit). The behaviour under test is correct; the assertion is order-by-position under concurrent work, which the robust-on-a-hot-machine rule forbids.

# Design

In rs/merge-queue/tests/cli_rerun_exempt.rs (a_failure_outside_the_named_checks_still_needs_its_second_sighting): select the batch, the confirming rerun and the base probe by identity (purpose plus changes plus commit) instead of by index: the rerun is the probe of the whole singleton batch at the batch's commit, the base probe is the probe of no changes at another commit. Keep every other assertion.

# Acceptance Criteria

The test passes under the parallel merge-queue suite on a loaded host (repeated runs), with no positional validation assertions left in it. cargo test -p merge-queue (all targets) passes.

# Notes

Evidence: a gated validate run relayed to this lane (one red case of 678 in rust.merge-queue.test; solo pass green). Reserved by the coordinator during the 2026-11-12 drain work, split from #1471 merge-queue-fake-dagrun-etxtbsy so each fix stays one coherent change.
