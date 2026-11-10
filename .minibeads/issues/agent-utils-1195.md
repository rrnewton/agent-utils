---
title: "reclaim-test-budget-sibling: the state-home reclaim success test exceeds the 60-second remove budget on a busy host"
status: closed
priority: 1
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T07:50:00.000000000+00:00
updated_at: 2026-11-10T07:40:00.000000000+00:00
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

## Resolution

Fixed (cl-census, 2026-11-10). Cause: this test's remove has a state home, so it runs two full /proc walks (the
remove's own census and the state-home census, `_stray_live_uses`) inside the default 60-second item budget. At
about 48 s per walk on a loaded gate host the budget runs out at `git rev-parse` or the post-fence scan. The test
checks the reclaim, not host speed.

Fix, tests only (the production `_AGENT_REMOVE_BATCH_ITEM_SECONDS` stays 60 s): in-process test removes run under a
test-only item budget, `_TEST_REMOVE_ITEM_SECONDS` = 3600 s, applied by the shared `_python_remove` helper
(`py/wrkslots/tests/test_remove_write_rust.py`) and restored afterwards. Unlike `--no-lock-budget` it keeps remove
on its default single-item path, so the remove differentials still exercise what production runs. Tests of the
budget itself pass `budget_seconds=None` and set their own, as #1188's
`test_remove_refuses_when_a_slow_census_spends_the_lock_budget` now does. The subprocess remove behind
`_interrupted_python_remove` sets the same test budget in its script.

Proof: `test_remove_reclaims_state_home_and_bundles_unpushed_git` now runs with the production item budget set to
1 s and the state-home census slowed by 1.5 s (#1188's `_slow_census`; the host's two ~48 s walks against 60 s,
scaled down), and the reclaim still succeeds. With the test budget taken out (`budget_seconds=None`) the same test
refuses with "this remove-agent-batch item used its 1-second budget for work before deletion at git rev-parse
--verify HEAD", the gate symptom. Local, 2026-11-10 02:20 ET: the test passed 5 runs in a row (46-52 s
each), the whole of test_reclaim_slot_state.py passed, and all 25 remove differentials passed.

Sweep: every test that drives a full remove under the default budget.
- Fixed, in-process through `_python_remove` (`_cmd_remove`): 13 of its 14 calls, 10 in `test_remove_write_rust.py`
  (the remove differentials) and 3 in `test_reclaim_slot_state.py` (the reclaim success test, `_make_orphan` for
  the reclaim-orphan-state tests, and #1188's state-home refusal test, which also passes `--no-lock-budget`), plus
  the subprocess behind `_interrupted_python_remove` (3 calls).
- Kept on the production budget on purpose: #1188's budget test (`budget_seconds=None`, item budget 1 s) and the
  budget tests in `test_lifecycle.py` that set `_AGENT_REMOVE_BATCH_ITEM_SECONDS` (0, 0.5 and 2 s) or assert the
  60-second message.
- Justified, unchanged: the CLI removes through `wrkslots.tests.idle_validation_host` in `test_lifecycle.py` (26
  calls, 7 already `--no-lock-budget`) and `orphan_retire_cases.py` (3, mostly `--no-lock-budget`), plus the 11 in
  `foreign_mount_ns_cases.py`, which run against a fake /proc (`FAKE_PROC`). None of them creates a state home, so
  each runs one census walk, under the 60 s budget at the measured ~48 s, and the lifecycle shards have stayed
  green in every gate the same night. Several assert the production 60-second message through the CLI, so a
  harness-wide test budget would change what they test. If one walk ever nears 60 s, the same test budget can
  move into `idle_validation_host` behind an explicit opt-in. No other remove-driving matches in the tests:
  `test_cli.py`, `test_retire_freeze.py` and the rest only loop over command names for help or refusal text, or
  drive the other edition's binary, which has no item budget.
