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

- Skip the census process's descendants as well as its ancestry. Build the parent map from the same listing (the parent pid is already read from each process's stat), and skip any process whose parent chain reaches the census's own pid. Compare start times along the chain, so a reused pid is never taken for a child.
- The caller's own children are never holders or agents: a remove's or an audit's own git commands, and the wrappers they run under (the tests' git stand-in included).
- Same rule in both editions.
- A deterministic differential on a fixture proc root (the `capture_process_snapshot_at` seam). A slot process that is the caller's child is ignored, one that is a grandchild through an intermediate process is ignored, and an unrelated process in the slot still counts. Both editions return the same verdicts.
- No new persisted wrkslots field.

# Acceptance Criteria

- The census skips the caller's descendants (start-time checked) as well as its ancestry, in both editions.
- The differential above passes in both editions.
- `test_other_edition_pools_rows_with_the_same_report` comes out of quarantine and passes in its namespaced shard in a gate.
- No wall-clock budget added (owner rule 346); no new persisted field.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2065 C1 gate.

Related:
- #2065: the hot-host census tests and the session git stand-in.
- #2418: the git helper race (orphaned helpers, which are no longer the caller's descendants).
- #1200: rust-status-consistency, the test's origin.
