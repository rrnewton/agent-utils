---
title: 'wrkslots-status-interrupted-mutation: status must report an interrupted mutation and its recover command'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T03:09:55.000000000+00:00
updated_at: 2026-11-16T03:09:55.000000000+00:00
---

# Description

Reported by a consuming team's lead on 2026-11-15. A host's ACTIVE journal
ended with a schema-2 `finish` record for one slot (validate_complete
false) whose actor PID was dead. `wrkslots status` returned 0 and said
nothing about it. The interrupted mutation surfaced only when a later
`finish` of an UNRELATED slot refused with "interrupted mutation recorded
... run 'wrkslots recover' first". `wrkslots recover --slot <slot>` then
completed it ("recovered finish: archived and removed").

Two defects:

1. status is silent about an interrupted mutation, so operators and
   watchers learn of it only when an unrelated command fails.
2. An unrelated mutation was refused by a dead actor's journal. #912
   wrkslots-dead-actor-journal (closed) was meant to stop exactly that, so
   this is either a regression or a path #912 did not cover (schema-2
   finish records with validate_complete false).

# Design

- status (human and --json) lists every interrupted mutation in the
  journals it reads: slot, mutation kind, schema, actor PID, whether that
  PID is alive (with the start-time identity check, not PID existence
  alone), how old the record is, and the exact recover command. A live
  actor reads as "in progress", not interrupted (#1121).
- status exits non-zero, or emits a documented warning field, when an
  interrupted mutation with a dead actor exists, so watchers can alert.
- doctor reports the same.
- Re-check #912 against a schema-2 finish with validate_complete false and
  a dead actor: an unrelated slot's mutation must proceed, or the refusal
  must name the blocking slot and the exact recover command. Fix whichever
  path fails.

# Acceptance Criteria

- A test journal ending in an interrupted finish with a dead actor: status
  prints an INTERRUPTED line with slot, kind, actor PID, alive=false and the
  recover command; --json carries the same fields.
- The same journal with a live actor reads as in progress.
- An unrelated slot's finish or remove proceeds past a dead actor's
  interrupted finish, per #912, or refuses naming the blocking slot and the
  recover command; a test pins whichever behaviour is chosen, with the
  reason in the Notes.
- Rust and Python editions agree where both implement status.

# Notes

Related: #912 (dead-actor journal must not block unrelated mutations),
#1121 (a live removal's journal reads as busy, not interrupted), #1701
(recover-absent memory thrash while holding locks).
