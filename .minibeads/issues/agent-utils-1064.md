---
title: 'test-temp-and-mount-leaks: pytest shard basetemps and failed image tests leak temp trees and loop mounts'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T14:25:00.000000000+00:00
updated_at: 2026-11-08T14:25:00.000000000+00:00
---

# Description

Two test-hygiene leaks found by disk passes on 2026-11-08:

1. `scripts/run_pytest_shard.py` creates a `mkdtemp` basetemp (`/tmp/agent-utils-pytest-<pid>-<n>-*`) and never removes it. One host had 347 such directories (22 GiB, 2026-10-30 to 11-07; since removed); another has 88 (1.1 GiB, 10-29 to 11-05).
2. wrkslots image tests leak loop-mounted ext4 images when they fail. One host showed loop mounts under `/tmp` and `/var/tmp` `pytest-of-<user>/garbage-*/test_dbg*/project/worktrees/slot-images/...`, and `losetup -l` listed about 11 loop devices whose backing files were deleted (e.g. `.../test_remove_checks_an_image_ba0/.../slot.img`). The other host has 2 such loop devices (`/tmp<random>/project/worktrees/slot-images/agent/slot01/{slot,state}.img (deleted)`). pytest's garbage rename cannot delete through a live mount, and a deleted backing file still holds its blocks.

Related: #1061 (a leaked chat run from a failed test held a slot's mount).

# Design

- `run_pytest_shard.py`: remove its basetemp when the shard ends; keep it only on failure AND when asked (an explicit flag or environment variable), and say where it kept it.
- Image tests: teardown that always unmounts and detaches what the test mounted (fixture finalizer / try-finally), on failure as well as success, logging each step.
- A sweep for dead test mounts and loop devices: only entries whose owning test process is dead, whose backing file is deleted or lies under a pytest basetemp, and whose mount point is under a pytest basetemp. Use `sudo -n umount` and `losetup -d` on each verified entry, never by pattern, and log each action with what proved it was a test leftover. Dry run by default.

# Acceptance Criteria

- A shard run leaves no `/tmp/agent-utils-pytest-*` directory behind on success, and keeps it on failure only when asked.
- A test that fails mid-way through an image mount leaves no mount and no attached loop device (a test forces the failure).
- The sweep, given a fixture with one live test mount and one dead one, detaches only the dead one, and its dry run lists it without acting.
- The existing leaked mounts, loop devices and basetemps are cleaned on both hosts with the sweep (verified, logged), and the freed space is reported.

# Notes

Filed by the devbig030 coordinator at the lead's request. Given to cl-start after #1061 chat-launch-outer-kill-leak (same family). The other host is reachable with `agentcloudctl node exec`.

## 2026-11-08 record: basetemps removed on devbig030 without a dry run

While building the shard sweep (cl-start), its first self-test ran against the real `/tmp`
instead of a private directory and removed 88 legacy `agent-utils-pytest-*` basetemps, 1.1 GiB,
with no dry run and no per-entry log. That broke the dry-run and per-entry-log rule; the
self-tests are hermetic since. The read-only survey taken just before stands as the record:

- 88 directories, 1.1 GiB in all, dated 2026-10-29 to 2026-11-05. Per day: 10-29: 2,
  10-30: 1, 10-31: 4, 11-01: 26, 11-02: 5, 11-03: 2, 11-04: 8, 11-05: 40.
- Largest: 207 MB (`agent-utils-pytest-1473233-0-gy9irb3g`), then 151 MB and 82 MB.
- Two live basetemps from 2026-11-08 09:06 PST (12:06 ET), whose wrappers were running, were
  kept.

## 2026-11-08 record: devbig030 loop sweep

`scripts/sweep_test_mounts.py` dry run, then `--apply` at 17:25 UTC (12:25 ET). It released 14
devices, one `sudo -n umount` per mount point and one `losetup -d` per device. Every exit code
was 0, and a dry run afterwards found nothing to release. The log, with the proofs for each
entry, is `~/.local/state/agent-utils/sweep-test-mounts.log` on that host.

- loop26 and loop27, under `tmpie6oiaqc`: backing files deleted; untouched for 142 h.
- loop30, 31 and 38-47: `tmpklfh_mw1`, `tmp3iiblekt`, `tmp04s2zu09`, `tmpv2_9cdib`,
  `tmpfp0bqe58` and `tmpe6oty_aj`. Backing files inside the root; untouched for 60-63 h.
- Left alone, because their names are not a test runner's: `probe894-*` and
  `probe894b/c/d-*` (loop28, 29, 32-37), and `repro955-*` (loop0, 1). These are manual
  experiments.

The released roots are Python `mkdtemp()` directories holding the `make_project` layout with
`add_recursive_submodules` and `slot_representation: image`. No test in the tree today combines
those, so they look like ad-hoc reproduction runs from 2026-11-02 and 2026-11-05. Their
directories remain: about 870 MB, mostly sparse images.

## 2026-11-08 devbig014 dry run (not applied)

26 devices would be released:

- 24 under `pytest-of-newton/garbage-<uuid>/test_dbg*/project/worktrees/...` (validate slot
  images and their fences). Backing files are deleted and no process references them. pytest
  renamed each of those sessions to `garbage-*`, which it does only to delete a finished
  session.
- 2 under `agent-utils-pytest-2121544-1-*/test_remove_checks_an_image_ba0`, untouched for
  3.5 h. That shard was killed before its teardown.

The first dry run there found two sweep defects, both fixed before any action:

- A `pytest-of-<user>` root was one directory, so the whole user directory counted as one test
  root. A root is now one session (`pytest-<n>` or `garbage-<uuid>`).
- pytest's repeated deletion attempts keep a garbage root's mtime fresh, so its age proves
  nothing. The rename to `garbage-*` is the proof instead.
