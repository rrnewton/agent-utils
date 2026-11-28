---
title: 'wrkslots-unmount-retry-test-real-proc-scan: test_unmount_busy_clears_on_a_bounded_retry scans every host process''s mountinfo and hit the 600 s per-test timeout'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- tests
- flaky-test
created_at: 2026-11-27T20:37:20-05:00
updated_at: 2026-11-27T20:37:20-05:00
---

# Description

[opus 5.5] A third test with the `#2372` wrkslots-image-tests-real-mount-scan cause, which `906b54fc` did not cover. `py/wrkslots/tests/test_worktrees_image.py::test_unmount_busy_clears_on_a_bounded_retry` was the only failure of python.wrkslots.shard-12 in the `#2396` gate on `0a5bd9ef` (2026-11-27, finished 7:18 PM ET): "_PerTestTimeout: ... exceeded its per-test timeout of 600s". The shard's other 185 tests passed.

The test fakes `worktreesimage._run` (umount busy twice, then success) and `slotimage.mount_table`, and calls `worktreesimage.unmount_bind(target, sleeper=waits.append)` with no `proc_root`. It does not use the `fake_proc_root` fixture that `906b54fc` added for the two tests `#2372` names. So each failed umount's holder recheck (`_wait_on_holder_recheck` -> `scan_mount_holders`) reads `/proc/<pid>/mountinfo` for every process on the host. The faulthandler dump at the timeout is inside that read. The host had roughly 120-140 thousand tasks at the time. The test's verdict therefore waits on the live host's process and mount count, not on the code under test.

No other gate log kept on the host (19 others) has this failure. The gate's rerun did not include shard-12; the `#2396` landing waits on a liaison step waiver for it.

# Design

Test-only (no production change):

- Run the test on the `fake_proc_root` fixture (or pass `proc_root=` a fixture process tree, as its siblings `test_unmount_busy_names_the_holding_pid_and_unit` and `test_unmount_transient_holder_disappearing_on_recheck_retries` do), so the holder recheck reads a few files and the fixture's guard fails the test if the scan reads any other root. The assertions (the retry succeeds after two busy answers, no holder diagnostic, `len(waits) == 2`) stay as they are.
- Audit every other test in the file (and in `test_lifecycle.py` image cases) that reaches `unmount_bind`, `scan_mount_holders` or `describe_mount_holders` without `proc_root` or the fixture. One such call is `test_unmount_bind_takes_deleted_peer_views_and_leaves_live_binds` (`unmount_bind(target)`, no proc root); check whether any of its paths scan holders and fake the root if so. Name each test checked in the notes.

# Acceptance Criteria

- `test_unmount_busy_clears_on_a_bounded_retry` reads only a fixture process tree, keeps its assertions, and finishes in seconds on a busy host; the fixture's guard fails it if it reads the host's `/proc`.
- Every test in the audit that can scan holders runs on a fixture proc root, or the notes show why its path never scans.
- The file passes 10 consecutive runs in its shard on a busy host, with load1 recorded.
- No production behaviour change, no new persisted wrkslots field, no wall-clock budget added (owner rule 346).

# Notes

Filed by `cl-transferlock` at the coordinator's request (2026-11-27), number reserved by the coordinator. Related: `#2372` (same cause, two other tests, fixture from `906b54fc`), `#2065` (the shard wildcard that labels it known-red), `#2396` (the gate that showed it).
