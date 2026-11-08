---
title: 'gate-flake-census-churn-abort: census evidence-changed aborts fail liveness tests under host load'
status: closed
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T02:20:00.000000000+00:00
updated_at: 2026-11-08T18:36:54.000000000+00:00
closed_at: 2026-11-08T18:36:54.000000000+00:00
---

# Description

Under full-gate host load, whole-host process censuses discard all three
attempt sets to unrelated process churn and refuse "relevant process
evidence changed during three liveness attempts". Tests whose assertions
require a completed census then fail in gates and pass alone:

- test_lifecycle.py::test_process_path_census_reports_a_live_user_while_unrelated_processes_exit_during_it
- test_lifecycle.py::test_validate_complete_still_refuses_live_path_use_after_owner_dies
  (refused with the churn text instead of naming the live process)

The census lane recorded identical failures on clean main when landing
its census scoping. Observed again in both #943 validation runs
(2026-11-07): each failure passes alone in seconds.

# Design

Scope the audit/remove census closure to target-relevant evidence (as
the same-UID census already closes on proven-unrelated rounds), or make
attempt aborts distinguish target-relevant identity changes from
unrelated churn, keeping holder detection fail-closed: a generation
that could hold a target path must still discard the attempt.

# Acceptance Criteria

- Both tests pass 20 consecutive runs under bounded contention
  (stress in a CPUQuota scope, or parallel gate-equivalent load).
- A live holder appearing/vanishing mid-census is still detected
  (existing census holder tests green).

# Notes

Filed under the #943 waiver (coordb, 2026-11-07). The reclaim
item-budget failure from the same gates is tracked by #954, not here.

Also seen 2026-11-08 under #997's load runs (cl-sync): test_audit_lock_free.py
test_audit_own_record_change_marks_only_that_row_unknown failed once in 4 file runs and once in 15
scenario replays (load average 40-90) because the audit's census refused with this churn text on
every row, marking the unchanged slot-b UNKNOWN.

Unexplained revert seen while preparing this change (cl-sync, 2026-11-08): between 12:39:53 ET
(the reflog's checkout of 94a79c91, the #997 landing, onto this branch) and 13:36 ET (the first
#996 commit), the three files #997 changed (py/wrkslots/tests/test_audit_lock_free.py,
.minibeads/issues/agent-utils-996.md and agent-utils-997.md) went back in the slot's working
tree to their blobs at d83ae91c, #997's base (test file 3cfc3f4d, agent-utils-997.md
f846743f), with HEAD unchanged at 94a79c91. They were restored from main before any gate or
landing; their modification times were lost in the restore. A read-only search found no writer:
gate.py, validate.py, the validation DAG files, the Makefile and common/bin make no checkout,
restore, stash, reset or read-tree call. dagrun's git sync and the merge queue's forced
checkouts run only in their own temporary or work directories. The audit test fixtures
(test_audit_state_rust._fixture and _git) build their repositories under tmp_path and strip
GIT_* from the environment; AU_ROOT is used only as the cwd of the read-only Rust parse-audit.
pytest basetemp and the cargo TMPDIR live outside the slot, so git discovery from a temporary
directory cannot reach it. The #997 scratch scripts edit only the test file, forward, and the
census tracer writes only its trace file. The one moment the tree held d83ae91c's blobs was the
`git rebase` start at 12:21:24 ET, before #997's own gate.

Remove-batch budget failures seen in today's gates (cl-sync, 2026-11-08; #954, which covered them,
landed as a99c66ca): test_reclaim_slot_state tests refused with "remove-agent-batch item used its
60-second budget" in 4 gate logs on devbig030, none of them touching that code: gate993r2 at
08:50 ET (1 test), gate955 at 14:34 ET (1), this change's first gate on ac8ef85f at 14:46 ET (4)
and mu-xdiff-gate2 at 15:56 ET (1). All passed when rerun. If it keeps recurring it needs an
issue of its own.

## Resolution

[opus 5.5 cl-sync] Traced under load (solo gate tickets, a scratch tracer logging every
evidence-changed refusal that escaped a census observer): the attempts were discarded by the
privileged find walk, "privileged find census produced diagnostics: '/proc/N/fd/N': No such file
or directory", where a process still running as its observed generation closed a file while
find walked its fd table. (The many "exited during mountinfo census" refusals are caught per
representative and discard nothing.) A census failed when that happened in all three attempts.

The diagnostics validator now raises a narrow subclass, `_LiveEntriesVanished`, naming those
processes when they are the only problem (no other diagnostic, no thread-group motion); every
other caller still discards the attempt. The find and maps observers read just those processes
again, at most twice (`_LIVE_ENTRY_REPROBES`), keep the batch's records of every other process,
and replace the interrupted walk's records of the re-read processes with the re-read's, as a
discarded attempt would; entries that still vanish refuse as before. This stays fail-closed: a
vanished entry of a live process means it closed that reference, a reference it kept or moved to
another descriptor is seen by the re-read, and a child handed the reference after the snapshot is
a birth the census never observes in any case. The Rust edition has no equivalent path: its process-use reads
(`rs/wrkslotsd/src/remove_write.rs` ProcessSnapshot, `supersede_write.rs`) read /proc links
directly and treat a vanished entry as no evidence, and its census paths defer to the Python
edition. Deterministic tests in `py/wrkslots/tests/test_lifecycle.py`. Load evidence:
20 runs of the two churn-sensitive tests (solo exclusive gate ticket, 12 busy-loop burners and
pytest pinned to 4 CPUs, load average 30-45) all passed on ac8ef85f; the tracer saw two
vanished-fd diagnostics of live processes, both absorbed by a re-read, with no attempt
discarded and no census refused (before the fix the same diagnostics discarded attempts).
