---
title: 'cpuset-cross-missing-exec-host-ledger: the cpuset differential case run:missing-executable-clean reserves a core in the host-wide reservation ledger, because it runs without a scratch ledger'
status: open
priority: 2
issue_type: bug
labels:
- cross
- dagrun
- validation
created_at: 2026-11-27T12:15:00-05:00
updated_at: 2026-11-27T12:15:00-05:00
---

# Description

In `cross/differential.py`, `compare_cpuset_alloc` runs `run:missing-executable-clean` as `cpuset-alloc run --cores 1 -- /definitely/missing/command` on both engines. It passes no `extra` environment.

**Why it reaches the host ledger.** The harness scrubs `DAGRUN_CORE_LEDGER` from every engine's environment (`_AMBIENT_CASE_CONTROL_ENV`). With nothing set back, both engines fall back to the host-wide reservation ledger, `$XDG_RUNTIME_DIR/dagrun/core-reservations.json`. Real work on the host shares that ledger.

**Both engines reserve before they exec.**

- Python `cmd_run` enters `reservation.reserve_cores(...)` before `_run_reserved_hard`.
- Rust `cmd_run` calls `reservation::acquire(...)` before `run_reserved_hard`, then `held.release()`.

So each validation run of `cross.dagrun.cpuset-differential` writes a short-lived reservation of one core into the shared host ledger, twice (once per engine), then releases it.

**This is not one of the live-scope cases.** It is not gated on `live_scope_allowed`, so it runs inside the delegated validation node too. #2388 gate-full-partial-delegated-live-scope moved the three live cpuset cases that had the same gap (`selftest:mutation-verdict`, `run:wrapped-help-passthrough`, `run:signal-status`) onto scratch ledgers. It left this case alone, because it was outside that change.

**What it costs.**

- A concurrent real reservation can see this core as taken for a moment.
- A crash between reserve and release would leave a stale record in the host ledger. The ledger's liveness check reclaims that record later.

The other `run` cases in the comparator (`run:missing-command`, `run:separator-required`, the invalid-invocation and IRQ-budget cases) are refused at argument parsing with exit 2, before any reservation. They don't write the ledger. This is from a code read, not measured.

# Design

- Give `run:missing-executable-clean` a scratch ledger, as #2388 did for the live cases: `_scratch_core_ledger(tmp)` in its own temporary directory, passed as `extra` to both engine runs.
- Check the whole comparator for any other engine call that can reach `reserve_cores` / `reservation::acquire` without `DAGRUN_CORE_LEDGER`, and fix those the same way. pin-run and interop already set it.
- Optional: a test-only guard in the harness. Every engine call that can reserve gets a non-empty `DAGRUN_CORE_LEDGER`, or the harness fails loudly. That would catch a new case with the same gap.

# Acceptance Criteria

- `run:missing-executable-clean` runs both engines with a scratch `DAGRUN_CORE_LEDGER`, and its verdict logic is unchanged.
- A test pins it. It runs `compare_cpuset_alloc` with fake engines that record each call's `extra` environment, and asserts that every call able to reserve carries a scratch ledger. Another test runs the case with a sentinel host ledger (`XDG_RUNTIME_DIR` pointed at a temporary directory) and asserts the sentinel is unchanged.
- `cross.dagrun.cpuset-differential` still passes, with the same check count.

# Notes

- Found by cl-shardcap while doing #2388 gate-full-partial-delegated-live-scope, which added `_scratch_core_ledger` and used it for the three live cpuset cases.
- Related: #2388; #28 delegated-cpuset.
