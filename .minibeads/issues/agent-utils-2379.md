---
title: 'wrkslots-all-process-census-ignores-gone-generations: the all-process census closes by ignoring generations that exit before their scan'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- census
- safety
created_at: 2026-11-27T08:40:00-05:00
updated_at: 2026-11-27T08:40:00-05:00
---

# Description

Safety. The all-process census (`_capture_process_path_census_following` and its follow-up listings in `_census_appearing_generations`, `py/wrkslots/cli.py`) counts a listing as proven when none of its new generations uses a target; a generation that exits while it is read is treated as "cannot be using a target" and does not block closure. That is the closure rule the 2026-11-19 cold review rejected for the same-UID census (`b1f7255e`, reverted by `8a1629b1`/`ad03f320`): a chain of fork-then-exit hand-offs, one per listing, gets past two proven listings, so the census can close while a descendant of a holder still uses the target.

Callers today: `_assert_validate_teardown_unused` (the B2-B validation teardown closure, where the recorded slice's empty subtree covers the slot's own work and this census covers everything else), `_assert_absent_slot_unused`, `_remove_validate_batch`, `_recover_finish_resume`, `_validation_checkout_facts` and `_cmd_audit_scan`.

Found while designing #2376 wrkslots-census-hot-host (design note `ai_docs/plans/2026-11-27-wrkslots-census-hot-host-design.md` in the harness repository, section (a), "The other closure rule already in the code").

# Design

Adopt the PID-cursor bracket from #2376 once that lands: probe every PID number the kernel allocates after the first listing and close only when every live process has been scanned clean and the cursor stood still for two whole probe steps. That keeps this census churn-tolerant (which the current rule was chosen for) while removing the hand-off gap. Each caller's own extra evidence (owner cgroup, slice subtree, Eden daemon exemption) is unchanged.

# Acceptance Criteria

- A hermetic test with an injected host: a holder forks its heir and exits before its own scan in each listing; the census never closes (today it closes).
- The same churn fixtures that close today still close (no new refusals from unrelated churn).
- Existing callers' tests pass unchanged; no persisted field changes.

# Notes

Filed by cl-census at coordb's direction (number assigned by coordb). Sequenced after #2376. Rust twin: #2380.
