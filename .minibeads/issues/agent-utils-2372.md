---
title: 'wrkslots-image-tests-real-mount-scan: two worktrees-image unmount tests scan every host process''s mountinfo and hit the 600 s per-test timeout on a busy host'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- flaky-test
created_at: 2026-11-27T06:44:00-05:00
updated_at: 2026-11-27T06:44:00-05:00
---

# Description

A sub-cause of `#2065` hot-host-census-tests, which covers the process census and fd holder scans but not this one. Two tests in `py/wrkslots/tests/test_worktrees_image.py` exceed the shard runner's 600 s per-test timeout on a busy host:

- `test_validate_mount_fence_refuses_when_the_unmount_fails`
- `test_remove_refuses_when_the_bind_unmount_fails_and_a_retry_finishes`

The stack in both: `unmount_bind` -> `scan_mount_holders` (each retry) and `describe_mount_holders` (the final refusal) -> `slotimage._parse_mount_table` over `/proc/<pid>/mountinfo` of EVERY process on the host. The tests stub `_run`, `_sleep`, `mount_id_at` and `mount_table`, but not the proc root, so their verdict waits on the live host's process and mount count. Measured on the gate host (2026-11-27, load1 131): 5,260 processes, 4,855 readable mountinfo files, about 5.0 million lines in all (about 1,021 per process), read alone in 16 s before any parsing; each test scans several times (`UMOUNT_ATTEMPTS` = 3 retries, the holder recheck, the final describe).

Sightings: gate `cd2417d4` on `48d31adf` (shards 2 and 4, finished 11:39Z, load1 about 120-180) and gate `2d0af2bf` on `5387cd55` (shards 12 and 15, finished 11:42Z, load1 120-236), the same two tests both times, both labelled known-red under `#2065`. The logs of the earlier wrkslots shard reds (`f775efd9`, `80db906e`) are pruned, so they cannot be attributed; `180b6bb0`'s were a different cause (an unplaceable deep path, `#2344`).

# Design

Test-only first (no production change without the coordinator's OK):

- Give each of the two tests a controlled proc root: a fake `/proc` under the test's temporary directory (or `worktreesimage._PROC_ROOT` pointed at it) holding exactly the holders the case needs, so `scan_mount_holders` and `describe_mount_holders` read a few files, not the host's thousands. The assertions about the refusal, its wording and the retry stay as they are.
- A guard: only the fake proc root's mountinfo files are read (count the reads), so a test that falls back to the real `/proc` fails at once instead of timing out.

For the coordinator, a production proposal (not part of the test fix): the holder scan is linear in processes times mounts on a big host. Processes that share a mount namespace (`/proc/<pid>/ns/mnt`) have identical mount tables; on the measured host 4,855 tables came from 129 namespaces. Parsing once per namespace (and testing each process's namespace only) would cut the work about 37 times without changing which processes are named as holders.

# Acceptance Criteria

- Both tests run against a fake proc root, finish in seconds on a busy host, and keep their assertions; a guard fails them if they read the host's `/proc`.
- They pass 10 consecutive runs in their shard on a host at normal load, with load1 recorded.
- No production behaviour change unless the coordinator approves the namespace proposal separately; no new persisted wrkslots field; no wall-clock budget added (owner rule 346).

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-27) as a sub-cause linked to `#2065`. Related: `#2299` (its gate's only reds were these two tests).
