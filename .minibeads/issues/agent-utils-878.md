---
title: 'wrkslots-legacy-validate-recover: audit and recover agree on a legacy absent validate row'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T09:32:58.474545063+00:00
updated_at: 2026-11-05T09:32:58.474545063+00:00
---

# Description

`wrkslots audit` and `recover-absent-validate-rows` disagree about a legacy validate row whose path is absent.

- `audit` reports the row as RECOVERABLE through `recover-absent-validate-rows --input FILE`.
- That command's plan refuses it with "no exact owner process generation". The row predates recorded owner generations.

An operator following audit's advice reaches a dead end. Found in a read-only registry triage on 2026-11-05.

# Design

Make the two agree, from one shared predicate:
- Either recover accepts legacy validate rows that carry no owner generation, under a safe rule. For example: the path is absent, the row's heartbeat has expired by more than its TTL, no live process holds the recorded cgroup or lease, and nothing authored is at stake (validate rows hold no work). Record the rule in the receipt.
- Or audit classifies such rows as not recoverable, and names the exact reason and the supported alternative.

Prefer the first if the safety rule holds, since the row is otherwise stuck forever.

# Acceptance Criteria

- One function decides "recoverable absent validate row". Both audit and recover-absent-validate-rows call it.
- Test with a fixture legacy validate row without a generation, with the path absent: audit's verdict and recover's plan agree (both accept, or both refuse with the same reason).
- Rows with a generation behave as today.

# Notes

From the lead on 2026-11-05 (P2). Assigned to mu-validate after #877.
