---
title: 'agentctl-python-shards-learned-cap-oom: python.agentctl.shard-0/1 are OOM-killed at their ~2.5 GiB learned memory cap and go unmeasured in full gates'
status: closed
priority: 1
issue_type: bug
assignee: cl-shardcap
labels:
- agentctl
- python
created_at: 2026-11-26T12:58:00-05:00
updated_at: 2026-11-26T20:29:00-05:00
---

# Description

`python.agentctl.shard-0` and `python.agentctl.shard-1` end as NO_RESULT in full gates: each is OOM-killed at its inner learned MemoryMax (cap about 2.5 GiB, peak about 2.5 GiB, 5 oom_kill events each) part-way through a single pytest process running 1457 selected tests per shard. Observed identically in gate `1795615981535044475` on `1e7b149d` and in gate `1795711962290625226` plus its exclusive rerun `1795715632490275083` on `6a13675b`, so this is current main behaviour, not tied to any one change under test.

Death points (2026-11-26): in the full gate, shard-0 died about 26% through its selection (about test 372 of 1457, 133 s in) and shard-1 about 30% (about test 443, 186 s in); in the exclusive rerun both died sooner (67 s and 111 s). Pytest prints only progress dots and the runner journal records no test boundary or culprit for these steps, so the evidence names no single test. The different death points, and the deterministic reproduction at lower host load, read as cumulative memory growth across hundreds of tests in one process running into a stale learned cap, not as one memory-bomb test and not as host memory pressure: the kill is at the step's own inner cap, and an outer/host OOM is reported in different words. Because the learned-cap NO_RESULT class is non-failing by design (owner rule 346, #1763), the only visible damage today is quiet: about 2900 agentctl Python tests go unmeasured in every full (`--all`) gate, and the gate verdict degrades to PARTIAL.

# Design

Three fix options, in increasing order of understanding required:

1. Re-profile the two steps so the learned cap rises to the footprint the shards actually need, as the steps' own NO_RESULT reason already prescribes. Cheapest, but the cap will chase growth again if the growth is a leak.
2. Split the selection further (more, smaller shards), so each pytest process peaks well under its cap. Bounds the blast radius and shortens each shard, at some extra startup cost per shard.
3. Find the growth: run a shard locally with per-test memory reporting (or periodic RSS sampling against the dot count), bisect the selection to the test region where RSS climbs, and fix the leak or unbounded cache if there is one. This is the only option that explains why two halves of one suite both outgrow 2.5 GiB within a few hundred tests.

Option 1 (with the new cap and its headroom recorded here) is the minimum to restore measurement; option 3 is wanted before the cap is raised a second time. Whichever is chosen, keep the NO_RESULT semantics: a learned-cap breach must stay non-failing, but it must stop being the routine outcome for these two steps.

# Acceptance Criteria

- `python.agentctl.shard-0` and `python.agentctl.shard-1` each complete with a measured pass (tests started and completed, no NO_RESULT) in a full gate on current main, on a busy host, with the learned cap and the observed peak recorded in this issue.
- The fix names its option (re-profile, split, or growth found and fixed) and, for a re-profile, the headroom between the new cap and the observed peak.
- No wall-clock pass/fail budget is introduced anywhere by the fix (owner rule 346).

# Notes

Filed from the #2280 S3 condition-(b) full gate on `6a13675b` (ticket `1795711962290625226`, PARTIAL solely because of these two NO_RESULT steps; every selected node passed, and the exclusive rerun `1795715632490275083` reproduced the NO_RESULT on both shards). The pre-existing proof is gate `1795615981535044475` on `1e7b149d`, which shows the identical NO_RESULT. S3 itself touches no Python or agentctl code. Related: `#1763` (learned-cap NO_RESULT semantics), `#2280 vibe-talk-state-sync-audit` (the gate that surfaced this).

## Resolution

Option 3: the growth was found, and it was a compile, not a leak. `py/tests/rust_binary.py` release-builds agentctl (and the fake chat plugin) for the shards' Rust differential cases, and it ran that Cargo build inside the shard's own pytest process. On a cold adapter target, that one rustc for the agentctl crate peaked at 2.45 GiB RSS at ab669389, 2.47 GiB at 68a3e938 and 2.69 GiB at abd810bd (with `CARGO_BUILD_JOBS=2`). The shard's cgroup peaked at 2.97 to 3.02 GiB uncapped, so the shard was killed at its 2.5 GiB cap at its first Rust case. Shards that completed before 2026-11-20 had found a warm adapter target and never compiled. From then on they compiled in every gate. The Python tests alone peak at 0.67 to 0.78 GiB, so the description's reading (cumulative growth across hundreds of tests) was wrong.

Landed as 45a8a48c (land-main replayed the gated f775efd9 from base d8cd2667). A new DAG step, `python.agentctl.build-rust`, runs `python3 py/tests/rust_binary.py` before either shard starts. It builds the same target with the same command, and both shards depend on it, so each shard's own Cargo call only checks freshness. The step's hint is a 4 GiB baseline, so its cap is 5 GiB: the 3.02 GiB peak plus max(25%, 512 MiB) (#818's rule) is 3.78 GiB, which leaves room for the crate's growth. The shards' hints are unchanged (2 GiB baseline, 2.5 GiB cap). No cap was raised and no shard was split. No wall-clock budget was added.

Evidence:
- Before, on abd810bd: both shards were OOM-killed at the 2.5 GiB cap. Uncapped, a shard peaked at 2.97 to 3.02 GiB, and an uncapped shard-1 passed all 1457 of its tests.
- Local dagrun run of the same patch, load1 159 to 176:

  | Step | Result | Time | Peak / cap | CPU |
  |---|---|---|---|---|
  | build-rust | PASS (cold) | 299 s | 2.76 / 5 GiB | 528 CPU-s |
  | shard-0 | PASS, 1458 passed | 626 s | 0.78 / 2.5 GiB | |
  | shard-1 | PASS, 1458 passed | 498 s | 0.67 / 2.5 GiB | |

  No rustc ran in either shard's cgroup.
- Owner-tier gate 1795735968490717874-2a1df79c on f775efd9 (full selection, 335 steps, on a busy host): build-rust passed in 0.6 s on a warm target. Shard-0 passed, 1458 passed in 899 s at a 640 MiB peak; shard-1 passed, 1458 passed in 1289 s at a 611 MiB peak. Both ran under the 2.5 GiB cap with no NO_RESULT. Eighteen steps outside the change went red under host load (wrkslots shards, three CPU-timeouts, vibe-talk browser checks). All eighteen passed on the exclusive owner-tier rerun 1795739990495742438-0ae8e637 on the same head.
- The whole agentctl Python suite on main's agentctl code: 2916 passed, 0 failed.

Follow-ups: #2317 makes repeated NO_RESULT runs and incomplete suites visible in the ledger and at landing. #2318 applies the same shape to the wrkslots and wrkviz shards, which build Rust inside their steps too. #2253 is closed as a duplicate of this issue.
