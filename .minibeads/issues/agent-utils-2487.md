---
title: 'queue-retire-test-hides-outcomes: the agentctl queue-retire test asserts that every entry was retired without printing the outcomes, so its one red names no cause'
status: open
priority: 2
issue_type: bug
assignee: cl-flaketriage
labels:
- agentctl
- tests
created_at: 2026-11-28T13:08:00-05:00
updated_at: 2026-11-28T13:08:00-05:00
---

# Description

`rs/agentctl/src/queue_retire.rs::tests::dry_run_lists_without_moving_and_all_pending_takes_every_lane` failed in one gate (head 668e988f, `rust.agentctl.test`, 2026-11-27) with:

    assertion failed: all.iter().all(|e| e.outcome == "retired")

The assertion does not print the entries, so the log cannot say which entry was not retired, or what its outcome was.

The test is single-threaded on a private temp queue (`temp_queue()`: the temp dir plus pid and a counter). Nothing in the log points to a shared resource.

# Design

- Step 1: make the assertion print every entry with its outcome (`assert!(..., "{all:?}")`; `RetireEntry` derives `Debug`), so the next red names its cause.
- Step 2, after the next red, or after a deliberate repro: fix what it shows.
- No wall-clock budget.

# Acceptance Criteria

- Step 1: the assertion message lists each entry's id and outcome.
- Step 2: the named cause is fixed, or shown to be a product defect and filed.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the 48-hour triage of `rust.agentctl.test` reds. Log read: the console gate log in that lane's scratch directory.
