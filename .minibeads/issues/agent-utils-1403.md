---
title: 'restore-drill-load-flake: restore-drill tests fail in rotation under host load in the gating agentctl shards'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-11T18:41:49+00:00
updated_at: 2026-11-11T18:41:49+00:00
---

# Description

[opus 5.5] Tests in py/tests/test_agentctl_restore_drill.py (#1133 restore-drill-proof) fail in rotation in the gating `python.agentctl.shard-0` and `shard-1` steps on a loaded host. A change that touches only chat latency paths (#1313 chat-latency-headless-accepted) failed three gate attempts and two solo shard reruns against current main. Each time it was a different restore-drill test, four tests rotating, while every other selected step passed (latency, Rust, the rest of agentctl). The latest failure was `test_restore_drill_one_unrestorable_lane_refused_at_74` (shard 0; shard 1 passed in that rerun). The other three rotating names come from the lane's gate logs and are added in Notes.

Under owner rule 346 (gating checks must be fast, high-signal and robust on a hot machine; anything else is disabled until fixed), these tests come out of the gate until they're load-robust.

# Design

- Quarantine the rotating restore-drill tests from the gating shards now, following the #1250 pattern: marked, tracked under this issue, and not run by gates.
- Make them load-robust: wait on events and conditions with generous timeouts (at least 5x solo), with no wall-clock budgets, and pass with the host above 75% CPU.
- Bring them back into the gating shards in the commit that proves it. The live restore drill doesn't depend on these gating tests.

# Acceptance Criteria

- The quarantined tests are named in the commit and in this issue, and gates no longer run them.
- Each returning test passes 20/20 in a saturated scope (the #1250 proof bar) and fails against a deliberately broken restore path.
- No wall-clock budgets in any of them.

# Notes

Owned by the restore-drill lane on devbig014 (#1133). Changes that don't touch restore-drill may land with a rotating drill failure as their only red while this is open, citing #1403, provided the failing test passes one local solo run. Numbered from devbig030's interim block (1400-1469).
