---
title: 'wrkslots-wrkviz-shards-build-rust-in-step: the wrkslots and wrkviz Python shards compile their Rust editions inside the shard, under the shard memory cap'
status: closed
priority: 2
issue_type: bug
assignee: cl-shardcap
labels:
- validation
- wrkslots
- wrkviz
created_at: 2026-11-26T17:56:00-05:00
updated_at: 2026-11-27T08:30:00-05:00
closed_at: 2026-11-27T08:30:00-05:00
---

# Description

#2306 found the agentctl Python shards OOM-killed because a test helper release-builds the agentctl crate inside the shard, so the shard's memory cap has to cover rustc as well as the tests. Two more components follow the same pattern:

- `python.wrkslots.shard-0` .. `shard-15`: `py/wrkslots/tests/rust_binary.py` runs `cargo build --release --locked -p wrkslotsd` (private target `rs/target/validation-adapters/wrkslotsd`, two Cargo jobs) from whichever shard reaches a Rust differential case first. In the dagrun profile stores since 2026-11-20 the shards peak at 94-99% of their 1.88 GiB cap on shards 0, 4, 12 and 14 (shard-0 at 1.87 of 1.88 GiB), so they are the next to cross, in the shard that happens to build.
- `python.wrkviz.shard-0` .. `shard-3`: module-scoped fixtures in `py/tests/test_wrkviz_*_rust.py` run `cargo build --release -p wrkviz` into the shared `rs/target`, without `--locked`, without a private target and without a Cargo job cap, so the compile width and the shared target's state depend on the host and on other steps. Peaks are up to 83% of the 1.88 GiB cap.
- `python.wrkslots.shard-3`'s 5 GiB memory hint cites `test_reanchor_liveness.py` running `cargo test -p agentctl` inside the step; that build moved to the Rust lane under #883, so the hint is stale and over-reserves memory in every gate.

# Design

Same shape as #2306's `python.agentctl.build-rust`:

- `python.wrkslots.build-rust` runs `py/wrkslots/tests/rust_binary.py` as a script (the same Cargo command, target and job cap the tests use), with a memory hint sized from its measured peak; every wrkslots shard that can reach a Rust case depends on it, so the shards' own Cargo calls only check freshness.
- wrkviz gets one shared helper (private target `rs/target/validation-adapters/wrkviz`, `--locked`, two Cargo jobs, built once per test process) that every `test_wrkviz_*_rust.py` fixture calls, and a `python.wrkviz.build-rust` step that runs it before the wrkviz shards.
- Re-measure `python.wrkslots.shard-3` without the old build and set its hint from that peak by #818's rule (peak + max(25%, 512 MiB)).
- Record each new step's measured peak and each shard's before/after peak in this issue.

# Acceptance Criteria

- Every wrkslots and wrkviz shard completes in a full gate with its Rust editions built by a separate step, and no rustc process runs inside a shard's cgroup when that step has run (checked by sampling the shard's cgroup during a run).
- The wrkviz Rust tests build into a private, `--locked`, job-capped target, not the shared `rs/target`.
- `python.wrkslots.shard-3`'s hint is set from a measured peak, with the measurement cited in the DAG comment.
- No wall-clock pass/fail budget is introduced (owner rule 346).

# Notes

Found by the in-step build audit done for #2306 (agentctl-python-shards-learned-cap-oom). Related: #818 (memory hint rule), #883 (bounded Cargo builds inside test shards; moved the reanchor build to the Rust lane), #2306.

## Resolution

Landed as 05197820 on 2026-11-27. The landing replayed 1b0c8419, which was gated on 37e6e6a9.

- **New steps:** `python.wrkslots.build-rust` and `python.wrkviz.build-rust`, as designed. Every wrkslots and wrkviz shard that can reach a Rust case depends on its component's build step.
- **wrkviz helper:** every wrkviz fixture shares one helper that builds into the private target `rs/target/validation-adapters/wrkviz`, `--locked`, with two Cargo jobs.
- **`python.wrkslots.shard-3`'s hint:** moved to the shared 1.5 GiB baseline (1.88 GiB cap). It was measured at a 1.03 GiB peak in a gate on f775efd9, and at 0.97 GiB in that gate's rerun. By #818's rule that needs 1.53 GiB. The DAG comment cites both runs.

**Evidence: gate `1795744310730890743-3856ce00` on faa276a2.** Same patch on an earlier base. A read-only cgroup sampler ran for the whole gate (5 s samples).

- **rustc in shards:** none in any of the 16 wrkslots shards, the 4 wrkviz shards or the 2 agentctl shards. All compiling happened in the three build steps.
- **Build steps, cold:**
  - wrkslots: 151 s, 258 CPU-s, peak 1.41 GiB (cap 2.5 GiB)
  - wrkviz: 153 s, 277 CPU-s, peak 1.30 GiB (cap 2.5 GiB)
  - agentctl: 272 s, 468 CPU-s, peak 2.74 GiB (cap 5.0 GiB)
- **wrkslots shards:** peaks 0.65-1.23 GiB against their 1.88 GiB cap, 0 OOM kills. Before this change, shards 0, 4, 12 and 14 peaked at 94-99% of the cap. Shards 3 and 7, on the shared hint, peaked at 1.21 and 0.91 GiB.
- **wrkviz shards:** 105-139 s, 84-106 CPU-s, peaks 0.42-0.54 GiB. Before: 169-575 CPU-s, peaks up to 1.39 GiB.
- **Reds:** `rust.agentctl.test` (the lock_rank panic, #1410) and `vibe-talk.check.read-aloud-blocks-browser` (on the known-red list; fixed by #2347). This change touches neither.

**Evidence: landing gate `1795777600642083805-50cbd6b7` on 1b0c8419.** Taken from dagrun's step table; no sampler this time.

- **Build steps, cold:**
  - wrkslots: 174 s wall, 298 user CPU-s, peak 1.5 GiB
  - wrkviz: 178 s, 317 s, 1.3 GiB
  - agentctl: 313 s, 538 s, 2.7 GiB
- **Shard peaks:** wrkslots 0.63-1.3 GiB, wrkviz 0.42-0.55 GiB, agentctl 0.74 and 0.63 GiB. 0 OOM kills, no NO_RESULT step.
- **Reds:** `python.agentctl.shard-0` (#2377) and `rust.merge-queue.test` (#2378). This change touches neither.
  - Their exclusive rerun, `1795784636031325182-279d838b`, passed.
  - The coordinator waived both as flakes for the landing.
