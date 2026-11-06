---
title: 'gate-ledger-true-base: a gate record should name the tree it actually ran on, not github/main at finish'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-06T19:18:02.000000000+00:00
updated_at: 2026-11-06T20:28:08.000000000+00:00
---

# Description

`scripts/gate.py` records `base_sha` in the gate ledger by resolving `--base` (usually `github/main`) when the run finishes. validate.py, however, diffs against the merge base of HEAD and that ref, and the tree under test is HEAD. On 2026-11-06 a lane copied "gate base 7c3a284a" from the ledger into a PUSH, but the gated tree's real base was f08298e5, 27 commits older. A replay with the reported base showed false overlaps, and the real distance to main was hidden.

# Design

- `head_sha` has the same flaw: it is read when the run finishes. On 2026-11-06 a lane committed in its slot while a gate ran, and the ledger filed that gate (base 5248dec8, 75 steps) under the new commit, which had never been gated. Resolve HEAD when the run starts, and record a separate `head_at_finish` when it differs, so a mid-run commit is visible.
- Record what the run used: `merge_base_sha` (the merge base of HEAD and the base ref, resolved when the run starts), `base_ref_sha` (the ref's value at start), and `tree_sha` (HEAD's tree). Keep `head_sha`.
- Keep `base_sha` for readers for a deprecation period, but make it the merge base, or document it clearly as the ref at finish.
- Have the PUSH and landing helpers read the merge base from the ledger.

# Acceptance Criteria

- A test: a commit made during a run leaves head_sha at the starting commit and sets head_at_finish.
- A test: a gate run on a branch whose base is behind the base ref records the merge base and the ref's value separately, both resolved at start.
- The ledger schema change is documented, and existing readers keep working.

# Notes

Requested 2026-11-06 by the lead coordinator.
