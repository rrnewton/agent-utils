---
title: 'refusal-cause-cleanup-line: a could-not-run cause must name the validate-lock refusal, not a later teardown line, and a fleet-wide quarantine is a host fault'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-11T13:30:00.000000000+00:00
updated_at: 2026-11-11T13:30:00.000000000+00:00
---

# Description

In `rs/merge-queue`, a donor validation that cannot start exits 75 (could-not-run); the queue records a one-line cause from `refusal_cause`, which takes the first line containing uppercase `REFUSED`. Related: #1294 donor-probe-pacing-bound.

What happened: a donor host job was refused twice, about 4 s each. The real refusal was a lowercase validate-lock line the record never showed: "validate-lock: cleanup quarantine: FLEET-WIDE validate quarantine: every validate-lock acquisition is refused ... this quarantine cannot be recovered by waiting". Validate-run's teardown then printed a `RETIRE-RETAINED ...` line, the first `REFUSED`-style capitals line, so the record named the teardown instead. Diagnosis was steered toward slot cleanup while a fleet-wide validate quarantine (cleared only by an attested recovery) blocked every validation on that host for about 23 h, recorded as one transient could-not-run after another.

# Design

In `rs/merge-queue`:

- `refusal_cause` names the refusal that stopped the validation: the first refusal from validate-lock admission or validate-run's own start gate. A teardown line printed after it (`RETIRE-RETAINED` or later cleanup) does not displace it.
- A quarantine refusal ("FLEET-WIDE validate quarantine ... cannot be recovered by waiting") is not transient: it maps to a host fault, so the donor stops claiming and an operator is told, instead of repeated could-not-run receipts.

# Acceptance Criteria

- A transcript with the lowercase validate-lock quarantine line followed by a `RETIRE-RETAINED` teardown line records the quarantine line as the cause; a test covers it.
- A quarantine refusal is treated as a host fault (the donor stops claiming), not a transient could-not-run; a test covers it.
- `cargo test --manifest-path rs/Cargo.toml -p merge-queue` (all targets) passes, including the new cases.

# Notes

P0 under the same-hour rule for merge-queue bugs. Reported 2026-11-11. Sequenced right after #1294 donor-probe-pacing-bound lands.
