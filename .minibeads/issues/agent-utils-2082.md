---
title: 'wrkslots-census-diagnostics: census stderr budget and timeout remedy text'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T18:35:00-05:00
updated_at: 2026-11-21T18:35:00-05:00
---

# Description

The validate-remove census caps its stderr at a fixed byte budget (observed values 1553 and 6651, unexplained), which truncates the evidence on a host with many processes, and its timeout remedy text names neither the slot nor the shape of the failure.

# Design

Document how the budget is derived; scale it with the process count up to a cap, or add a capped --census-stderr-bytes option; make the timeout message name the slot, the phase and the kind of failure (census not closed, unreadable process, budget exceeded) with the next step.

# Acceptance Criteria

- The budget and its derivation are documented and tested; timeout messages name the slot and failure shape.

## Refinement (mu-census, 2026-11-21)

The validate-remove stderr bound now scales with the live process
count up to a cap (`_validate_remove_batch_census_stderr_bytes`): the
historical 64 KiB per-slot floor, plus 256 bytes per process (the
absent-slot census's allowance, 16x the measured worst case of about
16 bytes per process at 4,400-5,500 processes), capped at 1 MiB per
slot. The derivation is documented at the function and tested
(floor/scale/cap). Census refusals name the slot and phase through
`_ReadOnlyCommandBudget.census_context` (wired for validate-remove),
the failure shape (time bound exceeded, budget exceeded, census not
closed, process evidence kept changing) and the next step; the Rust
not-closed text matches. No persisted fields, no new flags.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.

R3 (consumer proposal): the same-UID "did not close" refusals no
longer claim the 8 rounds were consecutive ("each of 8"); they report
the followed/relevant round counts out of all inventories and the
followed-generation total. Counter semantics unchanged. The
consumer's scripted short-lived-churn controls are vendored as
py/wrkslots/tests/test_census_short_lived_churn.py. The holder-found
naming and distinct exit status follow in a separate slice.
