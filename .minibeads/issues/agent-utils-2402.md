---
title: 'wrkslots-mount-holder-scan-fail-closed: the busy-holder scan counts an unreadable or malformed mount table as busy, and its de-duplication gets the missing tests and digest keys'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- safety
created_at: 2026-11-27T16:10:00-05:00
updated_at: 2026-11-27T16:10:00-05:00
---

# Description

These are follow-ups from the cold review of the #2372 production change (`scan_mount_holders` parses each distinct mount table once). That review approved the change: the de-duplication by table text is correct by construction, and no holder is missed relative to the old per-process scan. Both the old and the new scan, however, FAIL OPEN on bad input.

- **Unreadable table:** a process whose `/proc/<pid>/mountinfo` cannot be read (`OSError` other than the process having exited) is skipped. A holder whose table we cannot read is treated as no holder.
- **Malformed lines:** `slotimage._parse_mount_table` silently drops each line it cannot split. That is a line with no `" - "` separator, fewer than 5 fields before it or fewer than 2 after it. A table with a malformed line describing the held view therefore yields "not held". This also means the scan's `ImageError` branch for an unparseable table is unreachable in practice: the real fail-open is per line, not per table.
- **Tests:** there is no test for a table text shared by several processes that cannot be parsed, and none for copies of one table that differ only by a broken line.
- **Memory:** the de-duplication keys the verdict cache by the full table text. That held about 40 MB for one scan on a host with 170 distinct tables of about 240 KB each. The process-use census (`_MountTableVerdicts`) keys by a sha256 digest instead.
- **Documentation:** the root-only test `test_worktrees_image.py::test_remove_worktree_accepts_the_recorded_bind_root` is skipped unless `WRKSLOTS_ALLOW_ROOT_TESTS=1`. Coverage notes for the scan must name it as skipped, not count it as passing.

# Design

- **Fail closed.** In `scan_mount_holders`, any process whose table cannot be judged is reported as BUSY, an indeterminate holder named with its PID and the reason. That covers both an unreadable table and a malformed line in its table. It is not skipped, so the unmount refuses or retries as for a live holder.
  - A process that exited between the listing and the read (`ENOENT`, `ESRCH`) is still not a holder, as the census treats it.
  - `_parse_mount_table` gains a strict mode, or a companion that reports dropped lines, so the scan can tell "no entry for the view" from "a line was unreadable". Other callers keep today's behaviour unless reviewed separately.
- **Digest keys.** The verdict cache keys by `hashlib.sha256(text)` digest, like `_MountTableVerdicts`. The verdict stays a pure function of the text.
- **Tests:**
  - several processes sharing one unparseable text: every one is reported busy, and the text is parsed once;
  - copies of one table that differ by one broken line: the broken copy is busy, the intact copies are judged normally;
  - an unreadable table is busy;
  - an exited process is not;
  - the existing "same holders as the per-process oracle" test still passes.
- **Docs:** the holder-scan coverage note names the root-only skipped test.

This is a safety change in the removal and unmount path. It needs a cold review, and it must be coordinated with #2392 (inherited data-loss bugs in base removal) and lead-014's hold on removal changes.

# Acceptance Criteria

- An unreadable mount table, or a table with a malformed line, makes `scan_mount_holders` report that process as an indeterminate busy holder. Unmount then refuses or retries, with the PID and the reason named.
- An exited process is still not a holder.
- The tests above pass in the hermetic proc-root fixture of `test_worktrees_image.py` with no live host `/proc` read (owner rule 346).
- The verdict cache holds digests, not table texts.
- No new persisted wrkslots field. No wall-clock budget.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-27). This is an independent cold review of 50a174b4 (approve with nits) turned into one follow-up issue.

Related:
- #2372: the scan's de-duplication and the hermetic proc-root fixture.
- #2392: removal data-loss fixes.
- #2065: hot-host census tests.
