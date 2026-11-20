---
title: 'agentctl-cleanup-unproven-unlisted: harness-tree cleanup must list every pin it could not prove'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-20T08:08:18-05:00
updated_at: 2026-11-20T08:08:18-05:00
---

# Description

[watermelon-full-preview] Harness-tree cleanup drops pins it could not prove without naming them. In `py/agentctl/foreign/ownership.py` the census (about line 900) and `_pin_member` (about line 1204) return or skip unproven members silently; `rs/agentctl/src/foreign/ownership.rs` does the same (about lines 2063 and 1684). A `CleanupReport` can therefore read complete while members were left alone for lack of proof, and an operator cannot see which processes were never examined. Found by the cold review of #1938 agentctl-pidfd-open-einval on 2026-11-20.

# Design

- Every member the census cannot pin (unproven verdict, pidfd refusal such as EINVAL, vanished mid-check) is named in `report.unproven` with its reason, in both editions.
- Signalling behaviour is unchanged: unproven members are still never signalled.
- No persisted field changes (report any before gating if that changes).

# Acceptance Criteria

- Tests drive an unprovable member in both editions and assert it appears in `report.unproven` with a reason and is never signalled.
- Existing harness-tree cleanup suites stay green in both editions; a fully proven cleanup still reports no unproven members.

# Notes

Number reserved by coordb 2026-11-20 under the ask-first rule. Lane mu-agentctlfix. Related: #1938 agentctl-pidfd-open-einval (landed 776f69ff), #1952 agentctl-cgroup-events-read-error-fails-open.
