---
title: 'wrkslots-classify-batch-seals: report every lifecycle journal (kind, slot, owner PID, age), including validate-batch seals'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-08T23:40:00.000000000+00:00
---

# Description

While a validate-slot removal runs, its VALIDATE-BATCH-SEAL journal blocks every slot lifecycle command fleet-wide (py/wrkslots/cli.py, around the batch-seal check). An operator whose `adopt` or `create` is refused then has no single command that says which journal blocks them, who owns it, or how old it is. `classify-create-journals` reports only CREATE journals, and doesn't report their owner PID.

# Design

A classify-style, read-only report of ALL lifecycle journals (create, remove, validate batch seals, and the rest): kind, slot, owning agent, live owner PID (alive / dead / indeterminate), age, and what it blocks. Either extend `classify-create-journals` with an `--all-kinds` mode, or add a sibling command, with JSON output. Refusals caused by a journal should name that journal and point to the report.

This is the same family as #1086 validate-run-authority-budget (an orphan CREATE journal refusing every validation); it may be done by the lane that does #1086's follow-up.

# Acceptance Criteria

- With a live batch seal present, an `adopt` refusal names the seal and the report lists it with its owner PID and age (test).
- The report covers every journal kind the lifecycle writes, with a test per kind.
- Read-only: running it changes nothing.
- Rust parity, or the gap recorded.

# Notes

Requested by the lead (devbig014), 2026-11-08, as an addendum to a consumer's audit.
