---
title: 'wrkslots-recover-absent-stream-history: stream the event history in recover-absent-validate-rows and die with the launcher'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] Consumer measurement, 2026-11-19: a consumer's
validate-checkout cleanup has been off since 2026-11-15 because
`wrkslots recover-absent-validate-rows --apply` loads the whole wrkslots
event history into memory. On the reporting host, the machine event log
holds 91,232 files and 2.2 GB (0.99 GB on 2026-11-15), and the archive
snapshot is 300 MB. Under a 1 GiB MemoryHigh the command crawls
(memory.events high 226,466, oom 0; system CPU 4,010 s against user
22.65 s). For about 75 minutes it holds the machine ACTIVE lock, the
global lock and the state-root flock, so every wrkslots status on the
host fails.

#1701 (388834ae) chunked the commits to 64 rows and reports
`lock_seconds`, but every state load on this path still materialises
the whole event chain: `_load_events` builds a tuple of every parsed
event, `_states_from_events` folds from that tuple, and the event
writer loads the whole chain only to learn its tip. Whole-history
loads dominate; row count does not (measured in #1701: about
165-175 MiB at 200-1024 absent rows with an empty log, 216-269 MiB
with 20k-50k chained events).

Second defect: `recover-absent-validate-rows` is missing from
wrkslots' parent-death command list (`_PARENT_DEATH_COMMANDS`, #1681),
so when the launcher is killed by a timeout, the Python child is
orphaned and keeps the locks.

Reader skew constraint: a consumer pins an old wrkslots reader
(writer_ceiling 2). This change must persist no new field, file, or
event kind; journal shape, event kinds, and ACTIVE/ARCHIVED shapes
stay byte-identical. Reported to the coordinator before implementing;
approved with no lock narrowing (see Design).

# Design

- Stream the fold on the recover-absent-validate-rows path: fold the
  event chain one record at a time (the existing `_iter_events`
  verifies and yields each event, retaining nothing), so peak memory
  is bounded by current registry state, independently of history
  size. Keep the result identical: same derived state, same
  verification, same refusals.
- Position the event writer from the chain tip (count plus last
  digest, verifying the chain incrementally) instead of loading
  every event to find the tip.
- The archive snapshot file is read whole only when no event log
  exists; then it is the current state, not history. No
  whole-history archive read remains on this path.
- Locks: do not narrow. The batch safety proof is the durable prefix
  (archives form a prefix, ACTIVE removals form a prefix, both proven
  under one lock, #1701); releasing the locks between rows needs a
  new journal protocol, which is a persisted-format change and a
  reader-skew risk. Streaming shortens the hold instead; the
  coordinator approved this and no cold lock review is triggered.
- Parent-death: add only journal-resumable commands to
  `_PARENT_DEATH_COMMANDS`, starting with
  `recover-absent-validate-rows`. For each command added, a test
  kills the launcher mid-mutation (after the journal write, before
  completion) and shows a later recover completes cleanly and the
  locks are released.
- Rust edition (`rs/wrkslotsd`): its replay already streams one
  event at a time, but `recover_validate_write` re-loads the whole
  chain per row to match a recovery; stream that match. The plan/apply
  census path stays with the Python edition, as today. Run the
  existing differential parity suite.
- Measure on a synthetic history of at least 1 GiB generated under
  /tmp (never the consumer's event directory): peak RSS and wall
  time before and after.

# Acceptance Criteria

- `recover-absent-validate-rows` (dry-run and --apply) completes on
  a synthetic history of at least 1 GiB with peak RSS bounded
  independently of history size; before/after peak RSS and wall time
  are recorded in the resolution.
- Results are identical: existing absent-row lifecycle tests and the
  bounded-recovery tests pass unchanged, plus the Rust differential
  parity suite for this command.
- Killing the launcher mid-mutation kills the child and releases
  the locks; a later recover completes the batch cleanly. Tests wait
  on conditions, not wall-clock budgets, and pass on a host above
  75% CPU.
- No new persisted field, file, or event kind.

# Notes

Related: #1701 wrkslots-recover-absent-memory-locks (chunked commits,
lock_seconds; per-row lock release deferred there as needing a new
journal protocol), #1681 (parent-death signal for remove),
#1794 (streamed fold for read-only bounded scans), #1554 part A3
(refused recover-absent journals).

## Measurements (2026-11-20)

Synthetic history under /tmp (never the consumer's): 45,007 chained
events, 1.02 GiB, one absent validate row, real
`recover-absent-validate-rows --apply` in a subprocess
(ru_maxrss). Before (388834ae code): peak RSS 1,197.0 MiB, wall
15.28 s — over the consumer's 1 GiB MemoryHigh. After: peak RSS
54.2 MiB, wall 42.13 s on a loaded host (remaining streamed passes
each verify the chain; the pre-fix speed came from the in-process
event memo, which is itself the unbounded retention being removed).
Peak is now bounded by current registry state, not history size.

## Resolution

- Python: `_states_from_events_streamed` folds the chain one verified
  event at a time (shared `_fold_replay_event` with the materialised
  fold; tip-resumed within a memo scope under the hash-chain tip,
  the trust `_resume_replay` already uses). The recover-absent path
  uses it for global state, batch state, verification, and the
  batch-removal reload; pending-operations and hold scans stream;
  the event writer positions from an incremental chain verification
  instead of a whole-chain load. ARCHIVED is read whole only as the
  no-event-log snapshot (current state, not history).
- Locks not narrowed (coordinator-approved): the durable-prefix
  proof needs one lock (#1701); streaming shortens the hold.
- Parent-death: `recover-absent-validate-rows`,
  `recover-absent-agent-row`, and `recover-absent-agent-rows` added
  to `_PARENT_DEATH_COMMANDS` (all journal-resumable). Other
  long-running commands checked: remove family, retire-pending, and
  finisher were already present; non-journaled commands were not
  added. Tests freeze each command mid-mutation (journal durable),
  kill the launcher, and show the child dies, the journal survives,
  and a rerun completes.
- Rust (rs/wrkslotsd): `load_one_event` extraction plus
  `event_chain_matches`/`event_chain_fold`; the validate and agent
  recovery matches and the archive derivation no longer materialise
  the chain. Parity suites pass.

Tests: test_recover_absent_bounded.py (incl. streamed==materialised
fold), test_recover_parent_death.py (3 launcher-kill tests),
test_lifecycle.py absent subset (273 passed), Rust parity
test_recover_validate_write_rust.py + test_recover_absent_write_rust.py
(18 passed).
