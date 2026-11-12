---
title: 'team-restore-purge: inventory every restore path and delete the ones the team-restore driver replaces'
status: open
priority: 2
issue_type: task
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Owner, 2026-11-12: "We allow cruft a lot and should thoroughly overhaul streamline and purge old stuff for a clean and complete design." The trigger: a pinned desired-state list in restore.json (#742) silently overrode the registry-derived pool (#1129) and parked 26 lanes. See section 10 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- First an inventory of every restore surface (restore.json keys, restore flags, both editions, the host-wide wrapper, the boot units, report locations, watch and timer hooks), each marked keep, fold or delete.
- Expected deletions: the `agents` pinned list and `restore --write-agents`; the boot-id-only replay guard (replaced by #1405's epoch); per-team boot units folded into `agentctl-team@T.service`; the wrapper loop replaced by `team kick --all --wait`; the refusing Python restore stub if nothing calls it.
- Old keys get a hard refusal naming the replacement, plus one migration command per host. No deprecation shims.

# Acceptance Criteria

- The inventory is recorded in Notes before any deletion.
- After the purge, the restore user guide and the team-restore skill describe only the new design.

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).
