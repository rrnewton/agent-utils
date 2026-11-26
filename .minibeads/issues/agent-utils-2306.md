---
title: 'agentctl-python-shards-learned-cap-oom: python.agentctl.shard-0/1 are OOM-killed at their ~2.5 GiB learned memory cap and go unmeasured in full gates'
status: open
priority: 1
issue_type: bug
assignee: mu-statesync
labels:
- agentctl
- python
created_at: 2026-11-26T12:58:00-05:00
updated_at: 2026-11-26T12:58:00-05:00
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
