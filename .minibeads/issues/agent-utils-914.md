---
title: 'wrkslots-dead-seal-journal: a dead actor''s validate-batch-seal and paired finish journals self-recover'
status: open
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T01:30:32.061784973+00:00
updated_at: 2026-11-06T01:30:32.061784973+00:00
---

# Description

#912's dead-actor auto-recovery covers the absent-validate-rows journal, but not the `validate-batch-seal` journal kind or its paired FINISH journal. On 2026-11-05 at 7:06 PM ET, a consumer team on the lead host hit its third host-wide wedge of the day, on a tree that already contained #912 (d0067a4a):
- `VALIDATE-BATCH-SEAL.<host>.journal` was left by a ledger-publish unit whose actor pid was dead and whose systemd unit was gone. It was paired with a `FINISH.<n>...validate-fresh-<id>` journal from another dead pid.
- A harmless, unrelated mutation (`wrkslots hold <slot>`) refused: "interrupted validation-batch seal recorded in .../VALIDATE-BATCH-SEAL.<host>.journal; run 'wrkslots recover' first".
- A manual `wrkslots recover --coordinator-pid <pid>` cleared both: "recovered finish: archived and removed slot=..."; "recovered validation-batch seals: restored=0 resolved=1".

# Design

- Extend #912's provably-dead-actor recovery to the validate-batch-seal journal and its paired FINISH journal, with the same safety rule. The actor generation must be provably dead: pid plus start time no longer live, and no process left in its cgroup or systemd unit.
- Recover the pair together, in the order `wrkslots recover` uses, under the lock, and log what was recovered.
- If either actor might be alive, refuse as now, naming both actors and the recover command.
- Audit the remaining journal kinds (create, seal, finish, absent rows, validate-batch-seal, any others) and list in the issue which kinds now self-recover. The theme: no dead actor's journal should ever block unrelated mutations host-wide.

# Acceptance Criteria

- Test: a validate-batch-seal journal plus a paired FINISH journal, both from killed actors. The next unrelated mutation (for example `hold`) recovers both and succeeds.
- Test: the same with a live seal actor. It still refuses.
- Test: a seal journal whose systemd unit is gone and whose pid has been reused by an unrelated process is treated as dead, because start times differ.
- The issue lists each journal kind and whether it self-recovers.

# Notes

Reported by the lead on 2026-11-05 at 8:19 PM ET (P1), from a consumer team's third host-wide wedge of the day. Follow-up to #912 (dead-actor journal) and related to #885 (orphaned create journal). The exact outputs are in the lead's host notes; ask through the coordinator. Assigned to mu-wrkslots, right after #910.

Composes with #1121 finish-journal-live-remover (2026-11-09): a finish journal now names its removing process as top-level `owner` (a resuming `recover` rewrites it to itself). That is the identity this issue needs to prove the paired finish journal's actor dead: a live owner reads busy and must be waited out, and a dead one may self-recover with the seal journal. A legacy finish journal without `owner` gives nothing to prove dead and keeps today's refusal.

