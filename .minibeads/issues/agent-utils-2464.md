---
title: 'census-counts-own-descendants: the Rust process census skips only its own ancestry, so a pooled audit counts its own in-flight git children as running agents'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- hot-host
- census
created_at: 2026-11-28T08:22:00-05:00
updated_at: 2026-11-28T08:22:00-05:00
---

# Description

The Rust process census skips the process that runs it and that process's ancestors, but not its descendants.
- `capture_process_snapshot_at` (`rs/wrkslotsd/src/remove_write.rs:1247` on 2026-11-28 main) skips `own_ancestry()` (`remove_write.rs:728`: the process itself and its parents) and the coordinator pid.
- A child the caller started (a git command, and its own children) is counted like any other process. If its working directory is in a slot, the census sees "a live process uses the slot".

The audit captures ONE shared census for the whole run, lazily, from whichever row first needs it (`audit_state.rs:6458`, "rows judged in parallel wait for the one capture").
- With `--jobs N`, rows are judged in parallel, so the capture can happen while a sibling row's git command runs with its working directory in that row's slot. The census then counts the audit's own git child as a running agent in that slot.
- With `--jobs 1`, no git child is in flight when the census is captured.

Seen on 2026-11-28 in gate 1795870272526990146-f829ac09 (`python.wrkslots.shard-3`), in `py/wrkslots/tests/test_audit_lock_free.py::test_other_edition_pools_rows_with_the_same_report`, which compares `audit --jobs 4` with `--jobs 1`:
- the pooled report counted `running_agent_count` 1 and left `plain-0` out of the slots needing attention;
- the serial report counted 0, with `plain-0` needing attention.
- The session git stand-in was on (#2065: 5,557 git calls in their own namespace, none fell back), so this is not #2418's orphaned helper.
- The same case passed in gates 1795865238081247654-6865f313 and 1795865711756083419-854ca5d3 the same morning, and in every local namespace round: intermittent.

Remove uses the same snapshot function, so a remove whose own children are in flight during its census could count them as holders the same way.

The Python census also skips only its invoking ancestry (`ignore_invoking_ancestry`). Whether a Python path captures a census while its own children run needs checking in the fix.

# Design

- The AUDIT's census skips the audit's own descendants as well as its ancestry: `capture_audit_snapshot` / `capture_audit_snapshot_at` in `rs/wrkslotsd/src/remove_write.rs`, which the audit's one shared census now uses.
  - Each listing round builds a parent map from the same listing (parent pid and start time from each process's stat), and skips any process whose parent chain reaches the audit's own pid.
  - A child never starts before its parent, so a reused pid on the chain breaks it instead of adopting strangers.
- Remove's census is unchanged: `capture_process_snapshot` still counts the caller's own children, for two reasons.
  - Remove has nothing of its own in flight during its census: its git commands finish before the census runs. The orphaned helpers of #2418 are no longer its descendants, so this rule would not cover them anyway.
  - Remove's census tests use the test process's own children as the holders in a slot (`shared_snapshot_detects_live_use_and_allows_unused` and the fake-proc-root census tests). Those must still count.
- The Python edition needs no change. Its audit captures the shared census BEFORE judging any row (the `census_needed` pre-pass, then `_capture_process_path_census`), so no row's git child can be in flight at capture. The Rust audit captures lazily, inside the parallel row judging; that is where the race was.
- Tests:
  - Unit (Rust, fixture proc root): the audit snapshot ignores the caller's child and its grandchild through a live shell, and still names an unrelated process (a re-parented `setsid` sleep). The ordinary (remove) snapshot still counts the caller's own child. Without the unrelated process, the audit snapshot finds the slot unused.
  - End to end, through the Rust CLI: a test seam, `WRKSLOTS_TEST_CENSUS_OWN_CHILD_CWD`, keeps a child of the census's own process working in a named directory while each snapshot is taken.
    - Audit (`test_other_edition_audit_never_counts_its_own_child_as_an_agent`): with the seam in `plain-0`, `audit --jobs 1` and `--jobs 4` both report exactly what the audit reports without it, and an unrelated process in `plain-1` is still a live process in all three.
    - Remove (`test_remove_still_counts_its_own_child_in_the_slot`): with the seam in the slot, remove refuses for that live process and keeps the slot; its twin fixture without the seam is removed.
- No new persisted wrkslots field.

# Acceptance Criteria

- The audit's census skips the audit's own descendants (start-time checked) as well as its ancestry; remove's census still counts the caller's own children.
- The unit test covers both: the audit ignores its own child and grandchild and counts an unrelated process, and the remove snapshot still counts a test-child holder.
- The end-to-end seam tests pass: `audit --jobs 1` and `--jobs 4` ignore the audit's own child, and remove still refuses for its own child in the slot.
- `test_other_edition_pools_rows_with_the_same_report` comes out of quarantine and passes in its namespaced shard in a gate.
- No wall-clock budget added (owner rule 346); no new persisted field.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2065 C1 gate.

Related:
- #2065: the hot-host census tests and the session git stand-in.
- #2418: the git helper race (orphaned helpers, which are no longer the caller's descendants).
- #1200: rust-status-consistency, the test's origin.
