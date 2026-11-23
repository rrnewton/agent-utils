---
title: 'agentctl-turnover-snapshot-build-race: the daily turnover keeps finding a half-built snapshot and refusing'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T10:10:00-05:00
updated_at: 2026-11-23T03:26:59Z
closed_at: 2026-11-23T03:26:59Z
---

# Description

On three days running, the scheduled services turnover built its candidate and then refused because the snapshot directory for the same commit already existed but was incomplete. A second builder (likely the host-wide global agentctl build on the same morning timer) appears to create the same snapshot directory concurrently, so services miss their daily upgrade.

# Design

Make snapshot creation atomic and exclusive: build into a temporary directory and rename into place, take a per-snapshot lock, and treat a complete snapshot by the same id as success; an incomplete leftover from a dead builder is moved aside, not left blocking.

# Acceptance Criteria

- A test runs two builders for the same commit concurrently: both succeed against one complete snapshot; a leftover incomplete directory from a killed builder no longer blocks the next run.

# Notes

Filed by the coordinator; the owning lane refines this file.
