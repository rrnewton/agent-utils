---
title: "census-vanished-process: a process that exits mid-census makes slot use indeterminate instead of gone"
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-10T00:20:00.000000000+00:00
updated_at: 2026-11-10T03:00:00.000000000+00:00
assignee: cl-census
---

# Description

The process/path census refuses as indeterminate when a process exits while it is being read. Seen twice in one
full gate on 2026-11-09 (rust.wrkslots.test and repository.check.wrkslots-debug-test, both in
`tests::real_python_create_and_import_recoveries_close_their_marker_only_with_a_row`, wrkslotsd/src/tests.rs:6821):

    REFUSED: process use is indeterminate because /proc/<pid>/maps cannot be read: [Errno 3] No such process

A process that vanished between listing /proc and reading one of its entries no longer uses anything, so the
census should treat it as gone. #1102 wrkslots-census-churn-tolerant made the census tolerate churn, but this
path still refuses. A similar /proc race hit a cross-language differential gate step the same night, possibly the
same family.

# Design

- During the census, treat ESRCH and ENOENT on any `/proc/<pid>/*` read as "this process is gone": skip it and
  record it in the census diagnostics. Cover every per-process read: maps, fd (the directory and each link), cwd,
  root, exe and environ, and any other per-pid file the census opens.
- Keep "indeterminate" for EACCES, EPERM and every other error, so an unreadable live process still refuses.
- Python (`cli.py` around the "process use is indeterminate" sites, ~21233-21266) and the Rust census readers
  agree: same classification and same diagnostics.

# Acceptance Criteria

- Tests with an injected ESRCH and an injected ENOENT on each per-process read: the census completes and the
  process is not counted as a user.
- Tests with an injected EACCES: still indeterminate (refusal unchanged).
- A churn test (processes spawning and exiting during the census) passes repeatedly in both editions.
- The failing differential test above passes under load (5 consecutive runs, as a touched-test bar).

# Notes

Requested at P1 by the lead coordinator (2026-11-09).

## Resolution

Fixed in both editions (cl-census, 2026-11-09).

Named cause of the differential failure: #1102 part 1 made the remove path's direct /proc scan
(`_observe_slot_use_once`) read in full, through `_process_uses_slot`, every process generation born
after lsof ran. The six concurrent Python fixtures in
`real_python_create_and_import_recoveries_close_their_marker_only_with_a_row` keep starting and reaping
short-lived Git processes, so under gate load one of them exited between the open and the read of its
`/proc/PID/maps`, and `_process_uses_slot` refused that ESRCH as "process use is indeterminate".

Classification, both editions: a read of a `/proc/PID` entry that fails with ENOENT or ESRCH is no
evidence; the walk goes on and the census completes. ENOENT on one `fd/N` is that descriptor closed,
as before. EACCES, EPERM and every other errno on a live process still refuse, and scans that already
skip unreadable entries under lsof's rule are unchanged.

EACCES of an exiting process (found by the churn tests): once do_exit drops a process's mappings
(exit_mm), /proc makes its fd directory root-owned, so an exiting child's fd directory reads EACCES
even to its own user. But exit_files runs after exit_mm, so the process may still hold its
descriptors for a moment. `wrkslots remove` deletes data, so "no unseen holder" must hold exactly
(the lead's decision, 2026-11-09): an unreadable entry counts as gone only once the process has
exited. That means its stat is gone, or it is a zombie (State Z or X: exit_files has run by then;
a zombie leader whose threads still run is walked through those threads, as before). A process in
do_exit (PF_EXITING) is polled for at most `_CENSUS_EXIT_WAIT_SECONDS` = `CENSUS_EXIT_WAIT` = 200 ms
(10 ms polls) to become a zombie or vanish; if it is still exiting after that, it is
indeterminate and the census refuses, since a task stuck in exit (for example in D state) may still
hold the descriptors. Any other live process refuses at once, so a scan over many unreadable
processes never waits. An exiting leader with live threads keeps refusing.

- Python: `_link_target`, the fd directory and maps reads in `_process_uses_slot` (remove direct
  scan, supersede-handoff, registration owner check); census-only `_census_process_uids` and
  `_census_process_cgroup` in the direct scan, the agent-batch scan and the fenced scan
  (`_process_uids` keeps refusing a second ESRCH for its liveness callers); a reaped leader's status
  ESRCH in `_zombie_leader_thread_dirs` is a gone group, like ENOENT.
- Rust: `supersede_write::process_uses_slot`, whose link reader also swallowed EACCES and every
  other error. It now refuses an unreadable link of a live process like Python (unsafe-direction
  parity bug); the remove/audit snapshot (`read_process_links`) already treated ENOENT and ESRCH
  as no evidence and now notes them. The readers go through `proc_reads.rs`, which has a test-only
  error-injection seam.
- Diagnostics: there was no census diagnostics channel. With `WRKSLOTS_CENSUS_DIAGNOSTICS=1` both
  editions print `census: process PID exited during the census (/proc/PID/ENTRY: ERRNO); skipped`
  (or `thread T of process L (whose main thread has exited)`) on stderr for each process skipped this
  way, and `census: process PID still exiting after 200 ms (/proc/PID/ENTRY: ERRNO); indeterminate`
  when the wait runs out. ENOENT alone is noted only when the process has exited, so a kernel thread's
  absent exe is not. Documented in USER_GUIDE "Time-to-live and process evidence".

Tests. Python, `test_proc_status.py`: injected ENOENT and ESRCH on cwd, fd/3, the fd directory, and
exe+maps of real children whose only use of the slot is through that read (census completes, no use,
the ESRCH note), plus root; injected EACCES on cwd, root, exe, fd, fd/3 and maps of a live process
still refuses ("is unreadable"), at once when the process is not exiting; an unreaped zombie counts
as gone; a process whose stat shows PF_EXITING until it really becomes a zombie, within the wait,
counts as gone, and one held in PF_EXITING (fault-injected stat) past the wait refuses after at least
200 ms with the "still exiting" diagnostic; status and cgroup ESRCH are skipped; a churn test (200
children exiting while they are read). Rust, `supersede_write`
`census_gone_tests` and `remove_write` `census_gone_tests`: the same injections, EACCES and exit-wait
cases (a test-only stat replacement seam), the zombie case, a supersede churn test, and a snapshot churn test (2 threads each spawning and
reaping up to 100 paced children in the slot through 5 snapshots: never a refusal, and a real user of
the slot is always reported; bounded after an unbounded first version exceeded a gate step's CPU
limit). Results: the differential test passed 5 runs in a row
twice, under host load 138-189 (earlier rule) and 83-93 (final code, 2026-11-09 20:07-20:09 ET); the
new census tests passed 5 (Python) and 15 (Rust) runs in a row. Two races the tests found are fixed
in the change itself: a binary copied in-process could fail to exec with ETXTBSY when another test
forked (the test support copies it with `cp`), and deciding "exited" from two stat reads refused a
process reaped between them (one read decides).

Gap found on the way, out of scope here: the Rust remove/audit snapshot reads cwd, exe and fd, but not
root or maps. Filed as #1174 rust-remove-census-root-maps.
