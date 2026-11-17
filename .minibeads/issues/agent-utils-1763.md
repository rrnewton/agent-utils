---
title: 'dagrun-learned-step-memory-cap-too-tight: learned caps OOM light cargo steps under gate contention'
status: open
priority: 1
issue_type: bug
labels:
- dagrun
- validation
created_at: 2026-11-17T07:20:02.000000000+00:00
updated_at: 2026-11-17T07:20:02.000000000+00:00
---

# Description

[watermelon-full-preview] Gate step memory caps learned from profiles are too tight for
light steps under gate contention. `rust.dagrun.dagrun-model-deps` gets a learned cap of
about 83 MiB from about 55 MiB peaks; under gate contention its measured peaks rose
55 -> 60 -> 91 -> 153 MiB across runs (cgroup accounting includes parallel cargo children
and page cache), so it is OOM-killed (3 oom events in the latest validate on another
lane's tree, 2026-11-17 about 01:35 ET) and the gate goes red, though the same step
passes under the same cap standalone. This blocks the gate of #1583 and any gate that
hits that step under contention. Prior partial fixes: #818 put
`peak + max(25%, 512 MiB)` headroom in the opt-in censor-aware profile path only, and
#1704 raised this step's authored hard cap to 256 MiB; the default learning path still
derives `cap = 1.25 x p90 peak` with no absolute floor, and a learned-cap OOM still
fails the run.

# Design

- Learned cap formula, one owner in `dagrun.memory_feedback` (both editions):
  `learned_mem_cap_bytes(peak) = peak + max(25% of peak, 512 MiB)`. The ordinary
  profile path floors a store-derived RSS baseline at that value, so a learned cap is
  never below `peak + 512 MiB` for a light step. An authored `hard_mem_max_bytes`
  remains an explicit instruction and is never rewritten by learning.
- The two cargo-driving deps steps in `validation/rust.dag.yaml` keep an explicit hard
  cap sized from the contended peak: 153 MiB observed peak + 512 MiB floor, rounded to
  768 MiB, so a genuine runaway is still OOM-killed at a finite cap.
- Owner rule 346 mapping, documented choice: a memory-cap OOM kill at a LEARNED cap
  (no authored hard cap; cap derived from a profile baseline) maps to `no_result`,
  never `fail`. No retry is attempted: the scheduler has no per-step retry machinery,
  and a blind retry under the same contention would mint the same kill; `no_result`
  keeps the step visibly not-passed (dependents stay skipped, the profile row stays
  censored so the next learned cap rises) without minting a red. An OOM at an authored
  hard cap, or with no OOM event evidence, still fails.

# Acceptance Criteria

- Deterministic unit tests (fake cgroup event counters, no wall-clock budgets) pin the
  cap formula and the OOM -> no_result / fail mapping in both editions.
- The contended peak (153 MiB) fits the new caps with the stated headroom, and a step
  allocating far beyond its cap is still stopped by the kernel at that cap.
- Focused dagrun tests pass; the full local validate and the direct gate run are
  recorded under Notes.

# Notes

## Resolution

Implemented on `mu-dagcap/1763`:

- `dagrun.sizing.learned_mem_cap_bytes` (both editions):
  `peak + max(25% of peak, 512 MiB)`; the ordinary profile path floors every
  store-derived RSS baseline at that value, so a learned cap is never below
  `peak + 512 MiB`. Authored `hard_mem_max_bytes` is never rewritten.
- `dagrun.sizing.classify_memory_cap_oom` (both editions) maps fake-or-real
  cgroup `memory.events` counters: OOM at a learned cap -> `no_result`
  (owner rule 346, never fail; no retry, by the documented choice in Design),
  OOM at an authored hard cap -> fail, no OOM evidence -> not an OOM. Both
  schedulers mark the outcome `no_result`, skip fail-fast, report
  `NO_RESULT`/`PARTIAL`, and keep the profile row censored so the next
  learned cap rises.
- `validation/rust.dag.yaml`: both cargo deps steps' hard cap 256 MiB ->
  768 MiB (contended peak 153 MiB + 512 MiB floor, rounded).

Numbers (probe `/tmp/dagcap_numbers.py`): quiet p90 66 MiB -> old cap
82.5 MiB < contended peak 153 MiB (OOM); new learned baseline 578 MiB,
enforced cap 722 MiB (570 MiB headroom); fragment hard cap 768 MiB
(615 MiB headroom). Live probes on this host: a 2 GiB runaway under a
learned 80 MiB cap was kernel-OOM-killed at the cap and reported
`NO_RESULT` (run PARTIAL, exit 0); the same runaway under an authored
80 MiB hard cap was killed at the cap and reported FAIL.

Tests: `py/tests/test_sizing.py` and Rust `sizing` tests pin the formula
and the mapping with fake cgroup events; existing exact-value expectations
that encoded the unfloored learned baseline were updated to the floored
values. No wall-clock budgets.
