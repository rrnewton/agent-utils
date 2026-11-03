---
title: 'dagrun-memory-cap-headroom: profile-derived memory caps keep headroom above observed peaks'
status: open
priority: 2
assignee: mu-validate@devbig030
issue_type: bug
labels:
- dagrun
- validation
created_at: 2026-11-03T14:48:19.160164248+00:00
updated_at: 2026-11-03T14:48:19.160164248+00:00
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
