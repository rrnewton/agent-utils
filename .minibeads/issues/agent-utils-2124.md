---
title: 'agentctl-native-session-cross-binding: revive and adopt must refuse a native session already registered to another name'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T12:35:00-05:00
updated_at: 2026-11-22T12:35:00-05:00
---

# Description

On a consumer host, a recovery of one review seat's lost pane bound its name to another seat's native Codex session. A later revive of the first name failed with "native session is already registered as <other>; it was recorded for <first>". So two seats meant to be independent reviewers may have been one session for several reviews, which breaks review independence.

# Design

Every path that binds a name to a native session (revive, adopt, restore, any pane recovery) refuses when that native session id is already recorded for a different live or archived name, naming both; no silent rebind. Add a registry check (agentctl doctor or a dedicated command) that lists every native session id recorded under more than one name, with the time each binding was made, so past cross-bindings can be found. Identify which agentctl version or path performed the silent bind.

# Acceptance Criteria

- Tests: revive and adopt refuse a cross-binding; the registry check reports a planted duplicate; the path that bound silently is named and fixed.

# Notes

Reported by a consumer team; filed by the coordinator.
