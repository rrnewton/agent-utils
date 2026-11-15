---
title: 'validate-ledger-finisher: scheduled proof generation and abandoned-with-evidence terminal'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] 64 of 130 leaked slots on the evidence
host were validate checkouts retained because ledger publication
was never proven. `remove-validate-batch` requires an external
proof manifest plus completed record per slot, and no component
generates that proof on a terminal run or retries a failed mirror.
Deleting first would destroy the only retained evidence, so the
guard is right and the finisher is missing.

# Design

On a terminal validation run, emit the proof manifest and completed
record outside the slot; a scheduled step retries ledger
republication and then runs the existing validate removal path.
After bounded retries, an explicit abandoned-with-evidence-packed
terminal state (run record and artifacts packed outside the slot,
then removal) replaces silent infinite retention. The lifecycle
model (#1568) proves republication alone can fail forever; abandon
(or a mirror-succeeds assumption) is required for liveness.

# Acceptance Criteria

- A completed run's checkout is removed by the scheduled step once
  publication is proven, with the proof rechecked at deletion.
- A permanently failing mirror reaches the abandoned terminal with
  evidence packed and named, never a silent delete.
- Stale `running` records with dead owners are classified, not
  waited on forever.
