---
title: 'wrkslots-wrkviz-shards-build-rust-in-step: the wrkslots and wrkviz Python shards compile their Rust editions inside the shard, under the shard memory cap'
status: open
priority: 2
issue_type: bug
assignee: cl-shardcap
labels:
- validation
- wrkslots
- wrkviz
created_at: 2026-11-26T17:56:00-05:00
updated_at: 2026-11-26T17:56:00-05:00
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
