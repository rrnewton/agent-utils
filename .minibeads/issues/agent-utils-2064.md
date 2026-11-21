---
title: 'gate-defer-config-reload: a queued gate must re-read the defer config on every poll'
status: open
priority: 1
issue_type: bug
labels:
- gate
created_at: 2026-11-21T12:40:00-05:00
updated_at: 2026-11-21T12:40:00-05:00
---

# Description

gate.py reads the host defer config (gate-defer.toml) once, when the gate starts, and keeps those settings for the whole queue wait. After the host file dropped its unit-pattern and quiet-command deferral, gates queued earlier still deferred on the old unit list and quiet command for over an hour, behind work the host had decided to ignore.

# Design

Re-read the defer settings on every defer poll (and on admission), so a host-file change reaches gates that are already queued. A file that becomes malformed mid-wait keeps the last good settings and says so in the gate log, rather than dropping protection or aborting the gate.

# Acceptance Criteria

- A queued gate picks up a removed or added defer signal at its next poll, shown in its log.
- A malformed file mid-wait keeps the last good settings with a logged warning.
- Deterministic tests (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
