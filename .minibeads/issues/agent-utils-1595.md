---
title: 'proccpu-malformed-stat-skip: an unparsable stat for an unrelated task must not fail the whole CPU scan'
status: open
priority: 2
issue_type: bug
external_ref: gh-230
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

ProcessGroupCpu measurement scans procfs for group members. A stat read that succeeds but does not parse (for example an empty or truncated read of a task exiting under load) returns "malformed stat" and fails the whole scan, even when that task is unrelated to every measured group. Seen once in a full validation run (a launched sleeper became unmeasurable mid-test); retained logs and reruns otherwise pass. Both editions still propagate the parse error before the membership check; existing malformed tests cover only the group leader's own stat, which must keep erroring.

# Design

In both editions' scans (Rust KernelSource::consider, Python _consider), treat an unparsable stat like a gone task and skip it, after re-checking the pid is gone or the text is empty. Keep "malformed stat" for the group leader's pidfd-bound stat, where it is a real failure. Rust and Python change together with identical refusal texts.

# Acceptance Criteria

- Unit fixtures feed an empty and a garbage stat for an unrelated pid beside a valid group member and the measurement succeeds (both editions, shared corpus where applicable).
- The leader-malformed case still returns unavailable.
- A group member (non-leader) with a malformed stat is handled by whichever rule the design states, with a test naming it.

# Notes

[watermelon-full-preview] Filed from GitHub #230 proccpu-malformed-stat-skip (https://github.com/rrnewton/agent-utils/issues/230) during Request 098 triage. Remedy until fixed is rerunning the affected node. Distinct from #1597 proccpu-breach-breakdown (over-charging, not scan aborts).
