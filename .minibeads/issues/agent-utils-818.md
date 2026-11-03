---
title: 'dagrun-memory-cap-headroom: profile-derived memory caps keep headroom above observed peaks'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate@devbig030
labels:
- dagrun
- validation
created_at: 2026-11-03T14:48:19.160164248+00:00
updated_at: 2026-11-03T23:04:02.068423136+00:00
closed_at: 2026-11-03T23:04:02.068423006+00:00
---

# Description

[opus 5.5] python.wrkslots.shard-3 is killed by its memory cap in some gate runs and passes in others, unrelated to the change being gated. Diagnosis by mu-validate (2026-11-03): its hint rss_baseline is 1.5 GiB with no hard cap; the dagrun step profiles record genuine peaks of 2.13-2.18 GB, and the applied MemoryMax is about 2.13 GB, i.e. the cap has ratcheted to the observed peak with no headroom. One run passed (378 s); the gate run and an exclusive rerun were OOM-killed (5 oom_kills in 113 s). Every lane whose change selects that shard pays a rerun or a red step.

# Design

- Profile-derived memory caps mirror the owner's CPU rule: cap = observed peak plus max(25%, 512 MiB), never exactly the peak, and a cap never ratchets down to the latest peak alone.
- An OOM kill under a profile-derived cap is reported as a cap problem (re-profile; naming the step, the cap and the recorded peaks), not a test failure.
- Audit the other steps whose applied cap is within the margin of their recorded peak and list them.

# Acceptance Criteria

- python.wrkslots.shard-3 passes 10 consecutive gate runs on main without an OOM kill.
- A test shows a profile peak of P yields a cap of at least P plus the margin.
- The audit list of near-cap steps is in the issue.

# Notes

## Root cause (mu-validate, 2026-11-03)

[muse watermelon-full-preview] `python.wrkslots.shard-3` contains `wrkslots/tests/test_reanchor_liveness.py::test_real_agentctl_writer_records_reanchor`, which runs `cargo test -p agentctl --lib` INSIDE the pytest step. Measured uncapped inside dagrun, that file alone peaks at **4.2 GiB** (cargo compiling the agentctl test harness) and passes in 77 s; under the shard's authored 1.5 GiB baseline (cap ~1.9 GiB) it OOM-kills in ~24 s, and the whole shard grows to whatever cap is set (1.9 / 2.13 / 2.3 / hand-raised 3.0 GiB all recorded peak == cap, 4-5 oom_kills). Bisect proof under dagrun caps: shard halves -> q3 -> q3a -> q3a2 -> single file `test_reanchor_liveness.py` OOMs alone; the other four suspects pass singly. Whether a gate run OOMs depends on whether the agentctl sources changed since the last in-step compile (cold compile = 4.2 GiB peak; warm = ~1.1-1.6 GiB), which is why it passed 20 recorded runs at 1.15-1.63 GiB peaks and then failed repeatedly. The profile-derived cap path also ratcheted to the observed peak: `memory_feedback` added only 20% over the percentile and then took `max(..., observed, censored floor)` with NO margin over the maxima, so a step whose latest peak touched its cap was re-capped at exactly that peak.

## Fix

- `memory_feedback` (Rust + Python editions, cross-identical): estimate = largest considered peak (percentile, largest uncensored, largest censored) + `max(25%, 512 MiB)`, never exactly a peak; default margin 20% -> 25% plus `MIN_MARGIN_BYTES = 512 MiB`. Tests in both editions pin `P -> P + max(25%, 512 MiB)` literally, including the 512 MiB floor for a small peak.
- Scheduler (both editions): an inner-cgroup OOM is reported as `MEMORY CAP PROBLEM: [<step>] ... This is a cap problem, not a test failure: re-profile ... (peak + max(25%, 512 MiB))`, naming the step, cap and peak.
- `validation/python-components.dag.yaml`: shard-3 gets its own hint with `rss_baseline_bytes: 5153960755` (4.8 GiB, from the measured 4.2 GiB cold-compile peak; cap = baseline x 1.25), instead of the shared 1.5 GiB wrkslots anchor. Verified: shard-3 passes inside dagrun on the new cap.

## Near-cap audit (profile store on devbig030, 2026-11-03)

Steps whose applied cap minus recorded peak is below `max(25%, 512 MiB)` (worst first, excluding the /tmp bisect probes). Headroom 0 (peak == cap, censored): `repository.hygiene.wrkviz-schemas` (1.25/1.25 GiB), `packages.standalone.wrkslots` (0.80/0.80), `python.repository-infrastructure.cross-skip-accounting` (0.31/0.31), `python.wrkslots.shard-6` (1.99/1.99). Under 150 MiB headroom: `python.agentctl.shard-1` (39 MiB), `python.wrkslots.shard-5` (51 MiB), `python.wrkslots.shard-0` (82 MiB), `python.wrkslots.shard-4` (93 MiB), `python.wrkslots.shard-1` (122 MiB), `cross.agentctl.differential` (133 MiB over a 4.18 GiB peak), `cross.planner.differential` (43 MiB). Most remaining steps sit at exactly the old 25%-only margin (cap = baseline x 1.25) with < 512 MiB absolute headroom because their peaks are small; the new 512 MiB floor in the profile-derived path covers those when the feedback flag derives their caps. 76 step tags in the store fall below the #818 margin on at least one recorded row.

## Acceptance note

"10 consecutive gate runs" for shard-3 to be counted after this lands (gate runs on main); the single-step dagrun verification above is the pre-land proof.
