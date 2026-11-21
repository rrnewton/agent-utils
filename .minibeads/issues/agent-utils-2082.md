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

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
