---
title: 'agentctl-watch-false-reports: watch reports a non-agent directory and a missing restore report as problems'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

agentctl watch treats every directory under the registry's agents tree as an agent, so a directory without an agent record is reported unreadable. It also reports "no completed restore report for this boot" on a host whose boot predates per-boot restore reports while its agents are live.

# Design

Skip directories without an agent record; report a missing restore report only when a restore was expected this boot and agents are missing.

# Acceptance Criteria

- Neither false report appears in those situations; deterministic tests.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
