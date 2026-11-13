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

`capture_process_snapshot` delegates to an internal capture that takes the
process listing as a parameter; production passes the `/proc` listing. Tests
pass a listing of the test process and its descendants (`children` files), so
every process they assert about is one they spawned, and no host process can
enter the snapshot.

## Resolution

[watermelon-full-preview] Fixed with #1300 team-pid-namespaces Phase 2a:
`capture_snapshot_listing` takes the listing source; tests use
`capture_test_snapshot`. Product behaviour unchanged.
