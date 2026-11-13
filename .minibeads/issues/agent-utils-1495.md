---
title: 'revive-differential-proc-stat-parse-under-load: revive and restore treat a just-exited process as a stat parse error'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-13T17:20:00.000000000+00:00
updated_at: 2026-11-13T21:40:00.000000000+00:00
closed_at: 2026-11-13T21:40:00.000000000+00:00
---

# Description

Under load, revive and restore intermittently fail with "cannot parse process
stat for pid N" for a process that has just exited. Two gate observations:

1. `cross.agentctl.differential-2` showed 11 revive divergences rooted in
   "cannot parse process stat for pid N" under load (gate around 12:20 PM ET
   on 2026-11-13). The same step was 344/344 clean in a local validate.
2. A full gate's `python.agentctl.shard-1` failed
   `test_restore_drill_missing_slot_bind_refuses_only_that_lane`:
   "agentctl restore-teams: ... coord refused cannot parse process stat for
   pid 113179", so restore exited 75 where 74 was expected. It passed solo.

The message comes from `rs/agent-queue/src/identity.rs` `parse_process_stat`
(and any Python twin; search `py/` for the same parse). Hypothesis: a
`/proc/PID/stat` read that races the process's exit, or PID reuse, returns an
empty, short or zombie/teardown record, and it gets reported as a parse error
rather than "process gone".

# Design

Confirm the real cause first by reading the read path (`bounded_file` and its
callers) and reproducing deterministically: a fixture proc root holding
truncated or empty stat files, plus a real short-lived child read in a loop
under a condition, not a timer.

Fix: when the record is unreadable because the process exited (ENOENT/ESRCH on
open or read, empty read, or a record that vanishes on re-read), treat it as
"process gone" (not alive, not the recorded identity) wherever liveness or
identity is decided: restore, revive, the watch-agents liveness check and
agent-queue identity. Keep refusing a real malformed record from a live
process. Make the Rust and Python editions agree, and add a
differential/unit case for each shape.

Rules: no wall-clock assertions (owner rule 346); the tests must pass at
>75% CPU on 316 threads.

# Acceptance Criteria

- A fixture proc root holding empty, truncated, and vanished-on-re-read stat
  records is reported as "process gone" (not alive, not the recorded
  identity) by both the Rust and the Python editions, with a
  differential/unit case for each shape.
- A real malformed record from a live process still refuses with the parse
  error, in both editions.
- Restore, revive, the watch-agents liveness check, and agent-queue identity
  all observe the "process gone" outcome rather than a parse failure.
- Fresh cpuset local validate passes, then a 3-slot gate passes; a
  restore-drill red caused by this same stat parse is touched, not waivable.

# Notes

Filed by the mu-statrace lane from coordinator evidence; the issue file was
not yet on main when the lane started.

## Resolution

[watermelon-full-preview] Confirmed by reading the read paths and by
probing real short-lived children under CPU saturation: an exit race
surfaces as ENOENT/ESRCH at open, and the torn, empty, and
vanished-on-re-read shapes are what a fixture proc root (and an
in-flight teardown read) hands the same code. `bounded_file` turned the
unopenable and empty shapes into hard errors and `read_process_stat`
retried only parse failures, so the record of a process that had simply
exited was reported as "cannot parse process stat for pid N" (or an
unprovable probe), revive refused, and restore exited 75 for that lane;
the Python liveness twin raised the same shapes to "unknown".

Fix: the stat readers now classify at the read. A record that cannot be
opened (ENOENT/ESRCH), reads back empty, or vanishes on the bounded
re-read is "process gone" (not alive, not the recorded identity) in
`rs/agent-queue/src/identity.rs` (`read_process_stat`, the liveness
probe, `observe`), `rs/agentctl/src/foreign/ownership.rs`
(`read_stat_at`), `rs/agentctl/src/subagents/turn_runner.rs`
(`process_state`), and the Python twins `py/agentctl/client.py`
(`_liveness_process_stat`, which feeds the same verdict into
`process_liveness`) and `py/agentctl/foreign/ownership.py` (`read_stat`).
Liveness still requires the pidfd exit indication under a stable boot
before calling a vanished record dead. A record that stays readable and
unparseable still refuses as malformed, in both editions. Each shape has
a unit case in each edition (scripted readers, fixture proc roots, and a
reaped-child loop under a condition, never a timer).

Verified: focused Rust and Python suites for every touched reader pass;
fresh cpuset local validate (`validate: OK`) and a 3-slot gate
(`gate exit: 0`) pass on the rebased tree, including the restore-drill
case from the evidence and `cross.agentctl.differential-2`. Landed on
main as the agentctl commit "read a stat record that vanishes mid-read
as a process that exited" (see the landing notice to the coordinator
for the SHA).
