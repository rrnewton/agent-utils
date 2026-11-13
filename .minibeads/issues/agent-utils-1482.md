---
title: 'census-gone-tests-hermetic: remove_write census tests must not scan host /proc'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-13T00:00:00+00:00
updated_at: 2026-11-13T00:00:00+00:00
---

# Description

[watermelon-full-preview] The Rust `remove_write::census_gone_tests` snapshot
tests (and `link_tests::shared_snapshot_detects_live_use_and_allows_unused`)
captured the production process snapshot, which enumerates the real host
`/proc`. Any unrelated process on the host that is exiting with an unreadable
`/proc/<pid>/fd` then fails the test with "process use is indeterminate …
still exiting (PF_EXITING) after a 1001 ms wait", as seen in a gate run
(`a_mapping_is_a_use_and_unreadable_maps_follow_the_owner_rule`,
remove_write.rs census tests); solo, the same tests pass. The snapshot also
depends on the host's process population rather than only the processes the
test created.

# Acceptance Criteria

- The census tests take their snapshot over an injected process listing
  scoped to the test's own process tree, never the host-wide `/proc` scan.
- The product census rule is unchanged: production capture still enumerates
  the host table, unreadable evidence still refuses, and a process proven
  gone is still skipped with its census note.
- The previously failing mapping/owner-rule test passes by construction
  regardless of unrelated exiting processes on the host, with no assertion
  on host process counts.

# Design

Reuse the seam landed for the sibling gone-on-each-read test:
`capture_process_snapshot_at(proc_root, ..)` reads the listing, stat, thread
and link evidence under a caller-supplied proc root, and its test builds a
fake root holding only its fixture children. Convert the remaining census
tests to that same seam rather than building a second mechanism: each builds
a fake root aliasing only the processes it spawned, with fault injection
keyed at the fake paths.

## Resolution

[watermelon-full-preview] Fixed with #1300 team-pid-namespaces Phase 2a by
converting `a_mapping_is_a_use_and_unreadable_maps_follow_the_owner_rule`,
the processes-exiting churn test, and `link_tests::shared_snapshot_*` to
`capture_process_snapshot_at` over fake proc roots. Product behaviour
unchanged.
