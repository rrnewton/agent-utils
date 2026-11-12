---
title: 'wrkslots-validate-before-append: never append an event the registry loader rejects; recover-tail-event'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-12T14:30:00+00:00
updated_at: 2026-11-12T17:00:00+00:00
---

# Description

[opus 5.5 cl-wsrecover] On 2026-11-12, a trial create run from an unlanded branch against a live registry appended one active-state event: a flat-layout validation row whose checkout path was `<slot>/checkout`, not the slot directory itself. `_assert_record_paths` rejects that record at load. Every command replays the whole event log (`_load_active` -> `_states_from_events`, and the ACTIVE snapshot readers run the same check), so every later command refused, recovery commands included, and no verb could fix it. The registry stayed wedged until the event file was moved aside by hand under wrkslots' own mutation locks.

Two defects:

1. Writers never ran the loader's checks on the record they were about to append. `_write_active_state` only checked that the revision advances and that only the named slot changed; nothing checked that the new record replays.
2. There was no sanctioned way to recover from an unreplayable event. The hand repair had to take the locks itself, prove the tail was the culprit, move it aside, and rewrite the ACTIVE snapshot from the shorter log.

# Design

- Python: move the per-event bodies of the replay (`active-state-recorded`, `archive-state-recorded`) into `_fold_active_record_event` and `_fold_archive_record_event`. The replay and every writer then judge an event with the same code.
- Every active writer goes through `_write_active_state` (create, adopt, register, import, heartbeat, transfer, release, removal and recovery) or through `_write_active_rows_removed`. Both now fold the candidate payloads onto the state they advance from, after a JSON round trip, before anything is appended. The archive writers (`_write_archive_state`, `_append_absent_validate_archives`) do the same with the archive fold.
- A new or path-changed record also gets the current-layout `_assert_record_paths`, which the snapshot readers and the repository-backed replay apply to every active row. A refusal is a StateError prefixed "refusing to append an event the registry loader would reject: " followed by the loader's own message. Nothing is written.
- `_record_path_check_key` also carries the checkout's VCS, which the path rule reads.
- `recover-tail-event` is the sanctioned recovery for an event that is already in the log. The dry run proves the case: the local log's tail is the one event the replay rejects (with it the replay refuses, without it the replay succeeds), and it prints the sequence, the file sha256 and the loader's refusal. `--yes --sequence N --event-sha256 D --coordinator-authorized --coordinator-pid P --reason TEXT` then works under the mutation locks. It refuses while a journal is outstanding or a partial update is present, and when the dry-run values no longer match. It writes a receipt, rewrites ACTIVE and ARCHIVED from the shorter log, and moves the event file (nothing is deleted) to `<control>/quarantine/events/<machine>/<seq>-<sha16>.json`. The move comes last, so an interrupted run can be repeated. It refuses a tail that replays cleanly, a rejected event that is not the tail, and a log that stays unreadable without its tail.
- Rust: `ensure_states::append_event` runs `active_state::assert_appendable_active_payload` before it creates the event file. That parses the record as the loader does, checks its slot and machine, and runs `assert_record_paths` for a new or path-changed record. It refuses with the same prefix. The new verb stays with the Python edition, and the compatibility shell refuses it by name.

# Acceptance Criteria

- A new record and a changed record that the loader rejects (the flat `<slot>/checkout` shape) are both refused before they are appended. The event listing and the ACTIVE/ARCHIVED bytes stay unchanged, and `status` still loads. A valid heartbeat is still recorded.
- `recover-tail-event` sets aside exactly this shape: the dry run names the refusal; a stale sha256 is refused; `--yes` moves the file with its bytes intact and writes a receipt; ACTIVE equals the log without the tail; `status` and `heartbeat` work again. An interruption after the snapshot rewrite can be resumed by rerunning.
- It refuses a valid tail, a rejected event that is not the tail, an outstanding journal, and a run without `--coordinator-authorized`.
- Rust `append_event` refuses the same record with the same prefix and leaves the event directory unchanged; a valid record is appended and the log replays. The writer differentials (create, heartbeat, adopt, release, transfer, ensure-states, compat) still pass.

# Notes

Number reserved in the devbig014 ledger. Root cause and the hand repair that this replaces: the repair took the project-root, global and machine mutation locks, moved the tail event aside, and rewrote ACTIVE from the replay of the shorter log, which is what `recover-tail-event` now does.

## Resolution

[opus 5.5 cl-wsrecover] Landed as described in Design. Tests: the #1373 section of `py/wrkslots/tests/test_lifecycle.py` (writer refusals, valid change, tail recovery and its refusals, resume after an interruption) and the Rust unit test `append_refuses_an_active_record_the_loader_rejects`.
