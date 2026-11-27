---
title: 'dagrun-oom-learned-cap-no-result-masks-failure: a learned-cap OOM turns a real step failure into NO_RESULT and the run exits 0'
status: closed
priority: 1
issue_type: bug
labels:
- dagrun
created_at: 2026-11-24T07:45:00-05:00
updated_at: 2026-11-24T07:49:00-05:00
closed_at: 2026-11-24T07:49:00-05:00
---

# Description

From a cold gate audit (finding H5, HIGH): dagrun turns an OOM kill on a
learned-cap step into NO_RESULT even when the step really failed. Any
positive `memory.events` OOM counter (which counts kills by any OOM
killer, including host memory pressure) on a step whose cap was learned
from a recorded peak makes the step NO_RESULT, whether or not the OOM
caused the failure. NO_RESULT skips the failure path, its dependents
are skipped, the run result still reads ok, dagrun prints PARTIAL and
exits 0, and `scripts/validate.py` reads only the exit code and prints
OK. A learned-cap step that fails a real assertion while an OOM kill
lands on a helper therefore reads as success and can be landed.

Sites: `rs/dagrun/src/sizing.rs` (the learned-cap OOM classification),
`rs/dagrun/src/scheduler.rs` (the unconditional NO_RESULT conversion
and the failure path it bypasses), `rs/dagrun/src/cli.rs` (the run
verdict and exit status), the mirrored Python edition
(`py/dagrun/sizing.py`, `py/dagrun/scheduler.py`, `py/dagrun/cli.py`),
and `scripts/validate.py` (the exit-code consumer).

# Design

Fail closed and by name. An OOM-killed step whose command really
failed (an independent failure signal: a non-zero exit that is not the
OOM-kill termination itself, a timeout, or a structured test failure)
counts as FAILED, named as OOM-killed; its dependents are reported as
dependency-skipped because of it; dagrun exits non-zero; and
validate.py therefore never prints OK over it. NO_RESULT is kept only
for the genuinely-no-signal case (the step was terminated by the kill
itself, or exited 0 with no failure signal while an OOM landed): per
owner rule 346 a load event with no failure signal must not gate, so
that case stays PARTIAL with exit 0, but it is named per-step, in the
run summary verdict, and in validate.py's own PARTIAL line (never
"validate: OK"). No new persisted fields: the existing ok / no_result /
reason / skipped fields and the existing profile columns carry the
verdict, so older readers see the same schema.

# Acceptance Criteria

- A deterministic planted test (fake cgroup counters, a learned-cap
  step whose command exits non-zero, and a dependent) is red on the
  old code and green after the fix, in both the Rust and Python
  editions: the step is FAILED and named OOM-killed, the dependent is
  dependency-skipped, the run is not ok, and the CLI exits non-zero.
- Controls stay pinned: an exit-0 step with an OOM counter and a
  kill-terminated step with an OOM counter remain NO_RESULT (run
  PARTIAL, exit 0, steps named in the dagrun summary and in
  validate.py's PARTIAL line, never "validate: OK"); an OOM at an
  authored hard cap still fails; a plain non-zero exit with no OOM
  still fails plainly.
- A deliberately broken copy of the fix (the old unconditional
  conversion) is caught by the new tests.
- No wall-clock budgets are used as pass/fail criteria in the new
  tests.

# Notes

Pre-assigned #2207 dagrun-oom-learned-cap-no-result-masks-failure.
Owner rule 346 (a learned-cap breach under contention is not a
failure) is preserved for the genuinely-no-signal case; this issue is
about a real failure being masked, not about reclassifying the kill
itself as a product failure.

## Resolution

Fixed in both editions. A learned-cap OOM now becomes NO_RESULT only
when the step delivered no failure signal of its own
(`independent_failure_signal`: a non-kill non-zero exit, a timeout, or
a structured failure is a delivered verdict). With such a signal the
step stays FAILED, named OOM-killed, trips the failure path, its
dependents are dependency-skipped, and dagrun exits 1. A genuine
no-signal NO_RESULT is not a pass: dagrun prints PARTIAL naming the
steps and exits 3 (INCOMPLETE), validate.py prints
"validate: PARTIAL (incomplete: <steps>)" and passes 3 through (never
"validate: OK"), and gate.py records verdict "partial" with the steps
in a new `incomplete_steps` ledger field. Planted fake-cgroup tests
(exit 3 + OOM, plus a dependent) were red on the old code and are
green after, in both editions, and a mutant restoring the old
unconditional conversion is caught by them. No persisted schema
change: only verdict values, plus the additive gate-ledger field.
Landed on main as a617c1c6 under the liaison's standing waiver 965:
the gate row had no failed steps apart from the known-red
python.wrkslots.shard-9 family, and its only incomplete steps were
the pre-existing learned-cap NO_RESULT pair (#2306), which this
change surfaces as INCOMPLETE rather than masking.
