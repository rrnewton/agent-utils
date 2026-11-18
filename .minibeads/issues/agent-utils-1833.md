---
title: 'wrkslots-reopen-coordinator-reassert: transfer-owner --reopen never re-asserts its captured coordinator under the lock'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-18T00:05:24.000000000-05:00
updated_at: 2026-11-18T00:05:24.000000000-05:00
---

# Description

In _cmd_transfer_owner (py/wrkslots/cli.py:43688), the --reopen coordinator is captured before the lock (:43713) and its identity is written into the transfer event evidence (:43807) as captured; unlike every sibling capture in the same function (successor re-read and compared under the lock, owner re-proven), the coordinator is never re-asserted with _assert_caller_process under the mutation locks. A coordinator process that exits and whose pid is reused between capture and commit would be recorded as authorizing a reopen it never saw. Impact is evidence integrity, not state damage: authorization itself comes from --coordinator-authorized plus the caller's own checks, so this is hygiene, found by code reading while transcribing commands for #1828 wrkslots-transition-model-check (report section 7), not by the search.

# Design

Mirror the sibling pattern: re-assert the captured coordinator identity under the mutation locks before writing evidence, refusing if the process generation changed. Keep the change to that one re-assertion; the Rust transfer writer, if it has the same capture, gets the same treatment.

# Acceptance Criteria

A deterministic test with the coordinator generation changing between capture and commit (fake process identities, no sleeps) refuses the reopen and writes no event; the unchanged-generation reopen still succeeds with the same evidence shape. Fail before, pass after.

# Notes

[watermelon-full-preview] Filed from #1828 wrkslots-transition-model-check at the coordinator's request; see ai_docs/reports/research/2026-11-18-wrkslots-model-check.md section 7.
