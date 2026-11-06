---
title: 'package-contract-census-drift: the lifecycle partition check stops pinning test counts'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T08:49:54.492822744+00:00
updated_at: 2026-11-05T08:49:54.492822744+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

`python.repository-infrastructure.package-contract` was the worst repeat offender in the 24 h gate ledger to 2026-11-05: 4 first-run failures out of 60 gates. It is deterministic, not load.

`test_wrkslots_lifecycle_partitions_are_disjoint_and_complete` (py/tests/test_packaging_infrastructure.py) pins the exact size of the wrkslots lifecycle test set and its partitions, backed by a long comment that inventories every addition. So every commit that adds or parametrizes a wrkslots lifecycle test fails package-contract until it also edits this file. Typically the lane only sees that after a full gate. For example, #872 (b) hit 2133 vs 2131 and had to fix it in the same commit.

The check's real purpose is that every lifecycle test lands in exactly one partition: `ordinary | mapped == all`, the two disjoint, mapped_root ⊆ mapped. That needs no hardcoded totals.

# Design

- Keep the disjoint/complete/subset assertions; they find a test with a missing or contradictory mark.
- Remove the hardcoded totals and the per-addition comment inventory, or move any genuinely wanted floor (for example "at least N mapped_root tests") to a lower bound that does not churn on every addition.
- If a count must stay exact for a reason, the failure message names the reason, the expected and actual numbers, and the exact line to edit, so a lane can fix it from the first failure.
- Have the wrkslots shard steps (or `make validate`'s selection for py/wrkslots changes) run this check early, so the failure surfaces in minutes, not at the end of a gate.

# Acceptance Criteria

- Adding a correctly marked lifecycle test no longer needs an edit to test_packaging_infrastructure.py.
- A test with no partition mark, or both marks, still fails the check with a message naming the test.
- package-contract first-run failures from census drift drop to zero in the next 24 h ledger readout.

# Notes

From mu-validate's #827 readout preview (2026-11-05). Assigned to mu-validate after #873.

Moved from mu-validate to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684. The lifecycle partition check keeps its disjoint/complete/subset assertions with failures naming the tests, and no longer pins exact totals or the per-addition comment inventory; correctly marked tests need no edit there.
