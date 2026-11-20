---
title: 'agentctl-cgroup-events-read-error-fails-open: a cgroup.events read error must not read as an empty cgroup'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-20T08:08:18-05:00
updated_at: 2026-11-20T08:08:18-05:00
---

# Description

[watermelon-full-preview] After the kill wait in harness-tree cleanup, both editions read `cgroup.events` to decide whether the per-turn cgroup is still populated. In `py/agentctl/foreign/ownership.py` (`_kill_cgroup`, about line 744) any `OSError` sets `still = False`; in `rs/agentctl/src/foreign/ownership.rs` (`kill_cgroup_at`, about line 1310) `still` is `matches!(cgroup_populated(path), Ok(Some(true)))`, so any read error also reads as not populated. Either way the cleanup report claims clean (no remaining members) and the directory is removed while processes may still live in it. Unreadable evidence reading as death is the fail-open class #1938 agentctl-pidfd-open-einval forbids for pidfds. Found by the cold review of #1938 on 2026-11-20.

# Design

- A `cgroup.events` read error other than the directory being gone (ENOENT) reports the cgroup's members as remaining, never clean, and does not remove the directory.
- ENOENT (already gone) keeps today's clean path.
- Same handling in both editions; no persisted field changes (report any before gating if that changes).

# Acceptance Criteria

- Tests force a non-ENOENT `cgroup.events` read error in both editions and assert the report is not complete, members are named remaining, and the directory is not removed.
- The ENOENT path still reports clean.
- Existing cgroup and harness-tree cleanup suites stay green in both editions.

# Notes

Number reserved by coordb 2026-11-20 under the ask-first rule. Lane mu-agentctlfix. Related: #1938 agentctl-pidfd-open-einval (landed 776f69ff), #1953 agentctl-cleanup-unproven-unlisted.
