---
title: 'agent-utils-proc-scan-inventory: list every whole-/proc scan in agent-utils with a scoped replacement'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-22T06:05:00-05:00
updated_at: 2026-11-22T06:05:00-05:00
---

# Description

The owner asked for scoped notions instead of global /proc scans. Every remaining place that enumerates all processes (wrkslots censuses, agentctl watch and list, cleanup helpers) should be listed with its replacement.

# Design

An inventory document: each scan's call site, purpose, cost, and its scoped replacement (mount busy count, cgroup membership, pidfd, a recorded process identity), with an issue per replacement that is worth doing.

# Acceptance Criteria

- The inventory is committed under the agent-utils docs and each replacement has an issue or a stated reason to keep the scan.

# Notes

Filed by the coordinator from owner directions relayed by a consumer's lead; the owning lane refines this file as it works.
