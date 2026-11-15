---
title: 'wrkslots-lifecycle-model-check: checked lifecycle model, leak report, scheduled finisher'
status: closed
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Slots leak past their useful life: on one host
130 audited slots held 64 validate rows awaiting ledger proof, 27 dead
and deletable agent slots with no remover, and a long tail stranded by
coordinator-ancestry refusals, private submodule history, branch
drift, unbound owners, nested registrations, and global journals.
Two structural causes: (S1) no scheduled finisher consumes the
retirement queue or the TTL ladder, and (S2) read-handoff, remove,
and recover all require a descendant of one coordinator PID, so
delegation and coordinator restart strand every leased row.

# Design

- `py/wrkslots/lifecyclemodel.py`: single-slot exhaustive explorer
  (row, storage, owner truth/proof, heartbeat, handoff, hold,
  history, branch, nested, occupancy, journal, publication,
  coordinator generation). Baseline reproduces every observed leak
  class as a reachable state and a scheduled-liveness
  counterexample; the all-fix run proves dead, unoccupied,
  handoff-read slots reach archived under the finisher policy.
  `py/wrkslots/spec/WrkslotsLifecycle.tla` mirrors the same actions
  for a manual TLC run; the gate never fetches the jar.
- `wrkslots finisher`: dry-run plan by default (queued +
  expired-unhanded candidates, TTL teardown-rung candidates, every
  skip named); `--apply` delegates to `retire-pending` with all
  guards. Example systemd user units (dry-run report) and a tick-hub
  reminder (coordinator applies) ship in `py/wrkslots/examples/`.
- Follow-up fixes are filed separately: #1570-#1581.

# Acceptance Criteria

- `test_lifecycle_model.py` passes in the wrkslots component shards:
  every leak class reachable at baseline, baseline liveness fails
  per class, all-fix liveness holds, remove-path safety holds, the
  historical TTL bug and the audit hidden-history disagreement
  reproduce as counterexamples.
- `wrkslots finisher` dry-run mutates nothing; `--apply` without
  `--coordinator-pid` refuses; apply delegates to retire-pending.
- Leak report published with per-class code paths and fixes ranked
  by slots affected times ease.

# Notes

TTL teardown's missing occupancy/handoff gates (found by the model)
are #1576. The refused-apply journal defect is tracked with the
Eden absent-row work, not here.

## Resolution

[watermelon-full-preview] Delivered: `py/wrkslots/lifecyclemodel.py`
exhaustive explorer with `test_lifecycle_model.py` in the component
shards (baseline reproduces every leak class; all-fix scheduled
liveness holds; no safety violations), mirrored TLA+ text in
`py/wrkslots/spec/` (TLC run once by hand; the gate never fetches
the jar), `wrkslots finisher` (dry-run default, apply delegates to
retire-pending) with example timer units, and the per-class leak
report in the harness docs. Follow-ups #1570-#1581 filed. Unverified:
second-host class mix (rows pending) and first host enablement of
the finisher apply step (dry-run report review first, by design).
