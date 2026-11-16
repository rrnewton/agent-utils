---
title: 'wrkslots-validate-batch-census-bound-flag: let an operator raise the validate-batch census bound on a saturated host'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T16:24:09-05:00
updated_at: 2026-11-16T18:13:43-05:00
closed_at: 2026-11-16T18:13:43-05:00
---

# Description

[watermelon-full-preview] `wrkslots remove-validate-batch` refuses whole
batches on a CPU-saturated host: "read-only census exceeded its
operation-wide time bound (540 s, elapsed 564 s)". The bound is
`_validate_remove_batch_census_seconds` in `py/wrkslots/cli.py`:
120 s plus 60 s per further sealed slot, at most 8 slots (540 s).
The census is already one shared census per batch. The long comment
above those constants keeps the bound finite on purpose (stop-hook
callers; the seal window during which old clients refuse), so the
default must not move. What is missing is an operator escape hatch:
on a host under extreme load (load about 90, PSI some avg10 about
40) the census legitimately needs longer than the default, and
today the only remedy is to wait for the host to quieten.

# Design

- Add `--census-seconds SECONDS` to `remove-validate-batch`, and to
  the other commands that share the same census budget where
  trivial: `remove --validate-complete` /
  `--validation-proof-manifest`, `recover-ownerless-validate-batch`,
  and `classify-ownerless-validate-batch`.
- The flag may only raise the bound: a value below the computed
  default is refused with a clear message. Cap it at 3600 s.
  Default behaviour does not change.
- The census-timeout refusal names the flag: "rerun with
  --census-seconds N to allow a slower census".
- Document the flag in `--help` and in the wrkslots user guide;
  `scripts/check_documented_defaults.py` must pass.
- If the Rust edition implements `remove-validate-batch`, add
  parity; if not, say so in the report. (Recon: it does not — the
  command is Python-only; the ownerless batch commands refuse to
  the Python edition for their census, and `remove` refuses
  validate slots to Python, so no Rust census budget exists to
  raise. Recorded in the landing report.)

# Acceptance Criteria

- `--census-seconds` below the computed default refuses; above
  3600 s refuses; a raised value reaches the census budget.
  Flag-parsing and budget tests use a stubbed clock or budget,
  never a real wall-clock wait.
- Without the flag, every bound and refusal is byte-identical to
  today, apart from the timeout refusal naming the new flag.
- `remove-validate-batch --help`, `remove --help`,
  `recover-ownerless-validate-batch --help`, and
  `classify-ownerless-validate-batch --help` document the flag,
  and the user guide describes it.
- Deterministic regression tests cover the exact defect in the
  Python edition; no Rust change is needed (see Design).

# Notes

## Resolution

[watermelon-full-preview] Landed on main as
`528cf2ad6e4edcd0c435fbdd94b36c6b4a17ea38` (gated tree `0763b42e`;
owner-priority gate pass, local validate OK on the exact tree).
`--census-seconds SECONDS` now raises the shared census bound on
`remove-validate-batch` (computed default 120 s plus 60 s per further
slot), `remove --validate-complete` / `--validation-proof-manifest`,
`recover-ownerless-validate-batch`, and
`classify-ownerless-validate-batch` (flat 120 s default). Raise-only:
below the computed default refuses, above 3600 s refuses, defaults
unchanged without the flag. A census-timeout refusal names the rerun
("rerun with --census-seconds N to allow a slower census"). Documented
in `--help` and the user guide; `check_documented_defaults.py` passes.
Rust parity: none needed — the Rust edition does not implement
`remove-validate-batch`, the ownerless-batch recovery census stays
with the Python edition, and `remove` refuses validate slots to
Python, so no Rust census budget exists to raise. Tests use a stubbed
clock and budget only; the new defect tests fail on the parent code
and pass landed.
