---
title: 'wrkslots-status-interrupted-mutation: status must report an interrupted mutation and its recover command'
status: closed
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

## Resolution

Done (mu-wsfix). Status and doctor now report every outstanding
journal: `_journal_report` (Python `py/wrkslots/cli.py`; Rust
`rs/wrkslotsd/src/journals_state.rs`) parses the journal and checks
its recorded actor generation with the start-time identity proof
(`_process_state` / `process_state`), never PID existence alone.
Human status prints `INTERRUPTED slot=… kind=… schema=… actor_pid=…
alive=false age=… recover="…"` (a live actor prints `IN-PROGRESS`,
alive=true, per #1121); `--json` carries the same fields under
`interrupted_journals`. Status exits 3 when an interrupted mutation
with no live actor exists (2 still outranks it for a partial scan),
documented in the status help. Doctor adds `interrupted-mutation`
(and `journal-in-progress`) findings with the same line.

#912 re-check: `_recover_dead_actor_journals` auto-recovers only the
absent-row kinds, so a schema-2 finish with validate_complete false
was never covered. Per the coordinator's decision, a finish is never
auto-recovered (resuming it archives and removes the slot). Instead
`_assert_no_journal` takes `target_slot`: a finish journal provably
scoped to a different slot (slot agrees with its embedded record and,
for a scoped file, its filename) whose actor is provably dead is
stepped past with a stderr note; finish, remove, hold and unhold pass
their slot. The journal's own slot, and every global mutation
(target_slot=None), refuse naming the blocking slot and the exact
command `wrkslots recover --coordinator-pid PID --slot <slot>`. A
journal whose scope cannot be proven refuses the same way.

Tests: `py/wrkslots/tests/test_status_interrupted.py` (dead finish
reported in status human+JSON with exit 3, live finish in progress
with exit 0, doctor finding, unrelated hold proceeds while own-slot
and global gates refuse with the exact command and the journal is
left in place), the updated dead-actor wording in
`test_journal_actor_liveness.py`, and the Python/Rust differential
`test_status_interrupted_journal_matches_python` in
`test_status_cli_rust.py`.
