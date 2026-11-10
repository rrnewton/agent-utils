---
title: 'gate-cpu-containment: per-gate systemd CPU quota so gates cannot take the whole host'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-10T22:15:00+00:00
updated_at: 2026-11-10T22:15:00+00:00
---

# Description

A neighbouring merge-queue run was lost to KVM timeouts at host load 290 (2026-11-10, 5:08 PM ET); the top CPU users were validation gates' esbuild `--service` page builds (several at 116-303% each), node page tests, and Rust builds. The host gate cap was lowered to 2 shared gates as a stopgap. Gates must contain their own CPU so the cap can go back up.

# Design

- Each admitted gate runs in a `systemd-run --user` scope with `CPUQuota` set from a per-host value in the gate-slots file (`cpu_quota_cpus`, default 48 CPUs), so one gate's whole process tree is capped whatever its steps spawn.
- The admitted child environment caps the schedulers to the same share: `VALIDATE_MAX_CPUS` (which sets dagrun's `--max-cpus`) and `GOMAXPROCS` (esbuild's Go runtime) are lowered to the quota when they are unset or higher; a smaller caller value wins.
- `GATE_CPU_CONTAINMENT=off` disables the scope for a caller that cannot use it; when no user systemd is available the gate says so and runs uncontained rather than refusing to run.
- The quota is printed at admission and recorded so a gate's cap can be read back.

# Acceptance Criteria

- A CPU-bound command admitted through the gate measurably caps at the configured quota (peak CPU before versus after, recorded here), and an uncapped caller setting below the quota is honoured.
- Existing gate admission, rerun, nested-phase, blackout, and abort behaviour are unchanged (the gate test suite passes).
- One real gate's peak CPU is measured before and after on this host and recorded here.
- Lands on a normal gate.

# Notes

## Measurement (2026-11-10, this host)

Fixed-work CPU hog (16 workers, fixed loop count) through the scope mechanism: plain peak 438% CPU in 7.9 s; under `CPUQuota=200%` peak 173% in 21.3 s — the scope caps the tree at the quota (under the 200% ceiling; the host was heavily loaded during both runs). A hog admitted through `gate.py` itself with `GATE_CPU_QUOTA_CPUS=2` ran to a passing ledger record in its scope. One real full gate ran under this code on 2026-11-10: its scope showed CPUQuotaPerSecUSec=48s (48 CPUs) for the whole run.
