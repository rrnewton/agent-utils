---
title: 'census-fd-snapshot: a census test''s descriptor snapshot fails after another test leaks two pipes'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-19T09:35:54.440643709+00:00
updated_at: 2026-10-19T09:35:54.440643709+00:00
---

# Description

[opus 5.5] `py/wrkslots/tests/test_census_traversal_deadline.py` `test_default_budget_blocked_traversal_terminates_on_every_attempt_and_retries` failed in every CI run of the wrkslots shard that ran it straight after `test_census_threads.py` (2026-10-15, four runs): its final whole-process descriptor snapshot lacks four descriptors that were open before the test.

Likely cause, from reading (not reproduced): `test_census_threads.py` `_started()` kills and reaps its child but leaves both of the child's pipes to the Popen finalizer, and `test_thread_group_motion_does_not_skip_validating_later_diagnostics` binds `pytest.raises(...) as refused`, a reference cycle that keeps the test frame, the Popen and its two pipes alive until a full garbage collection. That collection likely runs inside the census test, which compares snapshots of every descriptor the process holds. Which tests share a shard with the census tests changes whenever test families are added, so any leaker can move this failure around.

Fix:
- close both pipes in `_started` (Popen as a context manager);
- take every census test's "before" descriptor snapshot after `gc.collect()`, so it holds only descriptors something still owns; keep every "after" read raw, so a census that leaves a descriptor only the collector would close still fails;
- a sentinel pipe opened before the traversal test's snapshot must still be open after its final census, which separates the test-isolation cause from production census code closing a descriptor it does not own.

Acceptance: a test shows `_started` leaves both pipes closed; a test shows the snapshot helper drops a descriptor held only by unreachable garbage; the census tests pass when run straight after `test_census_threads.py`. If a census test still loses a descriptor with these changes in, production census code is closing a descriptor it does not own, and that is a separate wrkslots bug.
