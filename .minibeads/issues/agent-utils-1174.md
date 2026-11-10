---
title: "rust-remove-census-root-maps: the Rust remove census reads cwd, exe and fd but not root or maps"
status: closed
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T00:30:00.000000000+00:00
updated_at: 2026-11-10T06:00:00.000000000+00:00
---

# Description

Found during #1173 census-vanished-process. The Rust edition's remove and audit census snapshot
(`rs/wrkslotsd/src/remove_write.rs`) reads each process's `cwd`, `exe` and `fd` entries, but not `root` or
`maps`. The Python remove census reads all five. So the Rust edition can miss a process whose only use of a slot
is a chroot root inside the slot, or a mapped file from the slot (including a deleted-but-mapped file), and
declare the slot unused.

Exposure: the Rust `remove` refuses per-slot image slots, and consumers currently run the Python remove path. But
plain worktree-slot removal in the Rust edition (the soak path) is exposed, and the gap must close before that path
is relied on.

# Design

Read the same per-process `/proc` entries as Python (cwd, exe, fd and its links, root, maps), with the same path and
(dev, ino) matching and the same error classification as #1173 (ESRCH/ENOENT = gone; EACCES and others =
indeterminate).

# Acceptance Criteria

- The Rust census reads cwd, exe, fd, root and maps.
- A differential test (both editions) where a process uses the slot ONLY through a chroot root, and another ONLY
  through an mmap of a file in the slot: both editions refuse to remove the slot.
- Existing remove differentials still pass.

# Notes

Filed for the lead coordinator; cl-census takes it right after #1173, as its own commit.

## Resolution

Fixed (cl-census, 2026-11-10). The Rust remove/audit snapshot (`read_process_links` in
`rs/wrkslotsd/src/remove_write.rs`) now reads each process's (and each surviving thread's) `root` link and its
`maps` table besides `cwd`, `exe` and the `fd` links, as the Python remove census does.

- `maps`: every distinct absolute mapped path becomes evidence, without the kernel's " (deleted)" suffix, split as
  the Python census splits a line (five fields, then the path, which may hold spaces; `mapped_paths`). Matching is
  by path, as for the other links (Python's (dev, ino) matching belongs to its fenced scan, which this snapshot
  does not implement).
- Error classification, per the lead's ruling (2026-11-10, (1') and (2')): ENOENT/ESRCH is gone (#1173; noted
  under `WRKSLOTS_CENSUS_DIAGNOSTICS=1`). EACCES/EPERM on a link, the fd directory or maps (`EntryReader::unreadable`)
  depends on whose process it is. For a process of the invoking user's real uid (`proc_reads::owned_by_invoker`)
  it means do_exit dropped its mappings, so the #1173 rule applies (`proc_reads::unreadable_entry_state`, one stat
  read): gone or a zombie, at once or within the census pass's exit wait, is no evidence; still exiting after that
  is indeterminate and refuses, naming the PID, its state (PF_EXITING, memory released) and how long it was waited
  for. A protected process that is not exiting (non-dumpable or setuid,
  e.g. a user systemd, keyring or privileged helper; 4 of 1,064 own-uid processes on the reporting host) and
  another user's process are no evidence, as Python's remove census takes them under lsof's rule. Any other read
  failure is undecided and refuses.
- Exit wait, per the lead's ruling (A) (2026-11-10): the first gate refused the remove differentials (shard-3, shard-5)
  with "/proc/PID/fd is unreadable" for own-uid processes still in do_exit past 200 ms. do_exit releases the mm,
  which makes the fd directory root-owned, then unmaps the whole address space before it closes the descriptors;
  for a large compiler or linker on a loaded host that took longer than 200 ms. `CENSUS_EXIT_WAIT` /
  `_CENSUS_EXIT_WAIT_SECONDS` is now 2 s in both editions, a TOTAL per census pass: one `ExitWait` /
  `_CensusExitWait` per snapshot (Rust remove and audit), per `_observe_slot_use_once` scan and per supersede
  successor walk, its deadline starting with the first exiting process, so ten exiting processes cost one wait and
  cannot stretch a lock hold (#1194). Rechecks start at 5 ms and double up to 50 ms, so an ordinary exit costs
  milliseconds. The wait ends early only when the process exits: once its memory is released the entry never
  turns readable again. A refusal after the wait reads "...; process PID is still exiting (PF_EXITING, its memory
  already released) after a N ms wait (this census waits 2000 ms in all for exiting processes)".
- Known gap, filed as #1192 rust-census-privileged-fallback (P1): Python's audit census re-reads such protected
  or foreign processes as root (`sudo -n`; `_same_uid_direct_path_processes` sends every process whose entries
  refuse this user to `_absent_validate_find_matches` / `_absent_validate_maps_matches`) and refuses when that is
  unavailable. The Rust snapshot has no privileged path, so Rust `remove` and `audit` take them as no evidence.
  Literal refusal instead would refuse every Rust removal on a host with a user systemd or keyring. Named in the
  USER_GUIDE (time-to-live and process evidence).

Tests. Differential `test_remove_refuses_a_slot_used_only_by_root_or_mapping_like_python[root|map]`
(`py/wrkslots/tests/test_remove_write_rust.py`), both editions: one child holds the slot only as its root (chroot
in an unprivileged user namespace, then fchdir back out), another only through a libc mapping of a slot file
whose descriptor it closed and which it then deleted. Both editions refuse and name the PID; with the new reads
taken out, the Rust edition removed and archived the slot in both cases while Python refused. The unreadable-maps
cases need injected faults, which only the test builds carry, so they are a paired test in each edition with the
same child and faults (sudo is never run): Rust `a_mapping_is_a_use_and_unreadable_maps_follow_the_owner_rule`
(remove_write census_gone_tests) and Python `test_census_unreadable_maps_follow_the_exit_and_lsof_rules`
(test_proc_status.py). Own-uid process still exiting past the wait: refuses after at least the wait with the
"still exiting" diagnostic and the PID, state and wait in the refusal. The shared deadline and the early end are
tested in each edition with stubbed stat reads: `exit_wait_tests` (proc_reads) and
`test_census_exit_wait_ends_as_soon_as_the_process_exits` / `test_census_exiting_processes_share_one_deadline`
(ten stuck processes cost one 2 s wait; the first is waited 2000 ms, the rest 0). Exiting within it: gone. Protected and not exiting: no evidence (today's documented #1192
behaviour, so #1192 flips a known test). Also the `mapped_paths` unit cases, all remove and audit differentials,
and the wrkslotsd lib tests.
