---
title: 'gate-memory-caps: wrkviz-schemas and the agentctl Python shards run out at their memory caps'
status: closed
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-04T20:00:09.647881000+00:00
updated_at: 2026-11-04T20:00:09.647881000+00:00
closed_at: 2026-11-04T20:00:09.647881000+00:00
---

# Description

Three gate steps hit their memory caps, which dagrun reports as a cap problem, not a test failure. They count toward #827 gate-flake-rate.

- `repository.hygiene.wrkviz-schemas` runs `cargo run -p wrkviz --example generate_archive_schemas`. When the wrkviz crate changed and no earlier step has rebuilt it in debug, the step compiles it itself, and rustc peaks above the step's cap (`rss_baseline_bytes` 1 GiB x `mem_cap_factor` 1.25). It was OOM-killed in #842 wrkviz-muse-backend-rust's gate, also when run alone with `--exclusive`. With the debug build current, it passes.
- `python.agentctl.shard-0` and `shard-1` were OOM-killed at their ~1.9 GiB caps (the lead's report). They share the 1.5 GiB `python_test_hint` baseline with eight other component steps.
- `python.wrkslots.shard-7` was OOM-killed at its ~1.9 GiB cap in this issue's full gate, again when run alone, and in another lane's gate the same evening.

# Design

Apply #818's headroom rule: cap = measured peak + max(25%, 512 MiB).

| Step | Measured peak | Needed cap | New baseline (cap) |
| --- | --- | --- | --- |
| wrkviz-schemas | 1,771,140 KiB (1.69 GiB; `/usr/bin/time` after touching `rs/wrkviz/src/lib.rs`; 1,733,116 KiB on main) | 2.20 GiB | 2 GiB (2.5 GiB) |
| agentctl shard-0 | 2,123,538,432 B (1.978 GiB, the largest of 111 recorded runs in the slots' dagrun profiles) | 2.49 GiB | 2 GiB (2.5 GiB) |
| agentctl shard-1 | 1,972,359,168 B (1.837 GiB, the largest of 109 runs) | 2.35 GiB | 2 GiB (2.5 GiB) |
| wrkslots shard-7 | 4.1 GiB with a cold Cargo build (a test in the shard runs Cargo; mu-validate's 16 GiB probe, 166 passed); 3,188,764,672 B (2.97 GiB) warm, run alone under a 5 GiB cap. OOM-killed at ~1.9 GiB in this gate and in another lane's | 5.13 GiB | shared 1.5 GiB -> own 4.25 GiB (5.31 GiB) |
| wrkviz-schemas-self-test | 0.02-0.09 GiB in recent runs (it writes synthetic schemas and never runs Cargo) | 0.59 GiB | unchanged (1.25 GiB) |

The agentctl shards and wrkslots shard-7 get hints of their own. The shared anchor moves, unchanged, to the next step that uses it, so the eight other steps keep 1.5 GiB.

# Acceptance Criteria

- These steps no longer OOM at their caps.
- The gate passes.

# Notes

wrkslots shard-7's peak grew from 1.3-2.0 GiB (2026-11-03) to 4.1 GiB: a test in the shard runs Cargo inside the step, so the peak depends on whether the build is cold. It is the #818 pattern seen in shard-3 (4.2 GiB cold compile), not a leak (mu-validate's analysis).

The same full gate's `python.wrkslots.shard-6` failed twice. The failures were different tests each time, but the cause was the same host evidence: the mount namespace of another live process was indeterminate, so a slot was judged unknown. It passed in another lane's run at 21:17 UTC, so it depends on the host, not on this change.

Numbered #851 because #850 is taken on another host. Toward #827 gate-flake-rate.

## Resolution

[opus 5.5] cl-start, 2026-11-04: the baselines in the table, with the measurements as comments in `validation/repository.dag.yaml` and `validation/python-components.dag.yaml`.
