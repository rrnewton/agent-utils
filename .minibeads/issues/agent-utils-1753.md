---
title: 'wrkslots-agent-batch-archive-ignored: remove-agent-batch cannot archive gitignored files and refuses them only after full salvage'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T22:35:05.000000000+00:00
updated_at: 2026-11-16T22:35:05.000000000+00:00
---

# Description

Owner-ordered host-wide slot cleanup: `wrkslots remove-agent-batch` has
no `--archive-ignored`. With the default ignored mode (`refuse`), salvage
warns about gitignored files and then refuses the slot
(`_PreparedRemovalRefused`), but only after the item's expensive salvage
and census work. In the field, 45 of 46 dead slots carry create-hook
provisioning output (local build config, generated build files, an
ignored directory), so a batch refused all of them, each after 70-100 s
of work: 0 of 11 removed. Single `remove --archive-ignored` works.

Two defects:

1. `remove-agent-batch` cannot archive gitignored files the way single
   `remove --archive-ignored` can; the batch item path refuses the
   flags outright.
2. When a batch is not archiving, the ignored-files refusal is decided
   only after the expensive salvage and census work, so a slot that is
   certain to refuse costs 70-100 s instead of seconds.

# Design

- Add `--archive-ignored` to `remove-agent-batch` with exactly the
  single-remove semantics for every slot of the batch: the same
  verified local archive, salvage receipts and read-back
  (`_archive_ignored_files`, `_assert_receipt_ignored_files`).
- When the batch is not archiving (and no finish-recorded ignored
  decision applies), decide the ignored-files refusal before the
  shared census and salvage work: probe each eligible slot's
  gitignored files outside the regenerable caches with the same
  collection salvage uses, and refuse the slot at once with the same
  refusal text. Slots the probe does not condemn proceed exactly as
  before, and salvage still makes the final decision for anything the
  probe cannot see (for example submodule checkouts).
- Git salvage / private-history subprocess cost is out of scope
  (#1437, another lane).

# Acceptance Criteria

- A batch whose slots hold gitignored files refuses them without the
  flag before any shared census or salvage work runs for them
  (deterministic counters, no wall-clock budgets), with the same
  refusal text as today.
- The same batch with `--archive-ignored` archives the files with
  read-back and removes the slots.
- Single `remove --archive-ignored` behaviour is unchanged, and a
  finish-recorded ignored decision (#1686) still governs its slot.
- `remove-agent-batch` does not exist in the Rust edition (it is in
  the `PY_ONLY` list of the Rust CLI), so the fix and tests are
  Python-only.
- Checks wait on conditions, not wall-clock budgets.

# Notes

[watermelon-full-preview] Field report behind the owner-ordered
cleanup: 45 of 46 dead slots carry create-hook provisioning output;
0 of 11 batch items removed, each refused after 70-100 s. Related:
#1751 (shared census cost), #1752 (referenced-ignored-paths warning
cost), #1302 (gitignored files outside the regenerable caches),
#1686 (finish-recorded ignored decisions).

## Resolution

[watermelon-full-preview] Landed on github/main as 44e74fb1
(gated 1272ff30 on 95db7464, replayed over d5f99889 with the
combined tree f43fb5f6 verified: the three batch ignored-files
tests pass there). `remove-agent-batch --archive-ignored` now
archives each slot's gitignored files with the single-remove
verified tar, receipts and read-back, and without the flag the
batch refuses a condemned slot before the shared census and
salvage, with the same refusal text; a finish-recorded decision
(#1686) still governs its slot. Tests:
`test_remove_agent_batch_refuses_ignored_files_before_census_and_salvage`
(census and salvage counters stay at zero) and
`test_remove_agent_batch_archive_ignored_removes_slots`.
