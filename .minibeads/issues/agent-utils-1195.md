---
title: "reclaim-test-budget-sibling: the state-home reclaim success test exceeds the 60-second remove budget on a busy host"
status: open
priority: 1
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T07:50:00.000000000+00:00
updated_at: 2026-11-10T07:50:00.000000000+00:00
---

# Description

`py/wrkslots/tests/test_reclaim_slot_state.py::test_remove_reclaims_state_home_and_bundles_unpushed_git`
(python.wrkslots.shard-5) runs a successful `remove` (`_python_remove(root) == 0`) under the default 60-second
item budget. On a busy gate host the process census alone takes about 48 s, so the remove refuses with "used its
60-second budget" at `git rev-parse` (pre-deletion checks), at the /proc scan after the path fence, or at
`git rev-parse --git-path MERGE_HEAD`. It fails on UNMODIFIED github/main 49304752 at host load ~52, reproduced as a
plain local pytest, as an exclusive solo rerun and in a full gate (evidence appended to #1188 in 5694af68). So every
gate that selects the wrkslots shards is red on this test, whatever the change. #1188 fixed the sibling refusal test
the same way; this test still depends on host speed.

# Design

The test checks reclaim (state home removed, unpushed git bundled), not the budget. Run its remove with
`--no-lock-budget` (or a test-only budget well above the host's census time), as #1188 did for the refusal test.
Sweep the rest of test_reclaim_slot_state.py, and the other tests that drive a full remove under the default budget,
for the same pattern; fix them all in one change. Keep the budget-specific tests from #1188 as they are.

# Acceptance Criteria

- The test passes on a loaded host: a census slowed past 60 s (as in #1188's tests) still lets the reclaim succeed.
- The sweep is listed in the Resolution: every remove-driving test using the default budget, fixed or justified.
- No production default changes.

# Notes

Reported by the lead coordinator (2026-11-10 2:45 AM ET) from mu-gateprio's #1190 gate; filed as its own issue
rather than reopening #1188, which is closed with its own delivered fix. P1 because it reds every wrkslots gate.
Assigned to cl-census (census lane), ahead of #1175.
