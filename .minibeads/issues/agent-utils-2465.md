---
title: 'lifecycle-recovery-cases-outside-gates: 27 test_lifecycle.py frozen/ownerless-validate recovery cases fail outside a gate; establish per case whether the environment or the code is at fault'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- lifecycle
created_at: 2026-11-28T08:41:00-05:00
updated_at: 2026-11-28T08:41:00-05:00
---

# Description

27 `py/wrkslots/tests/test_lifecycle.py` cases fail when run outside a gate, from a lane's scratch temp root. `cl-census` ran them at head 8740b2e1 on 2026-11-28 (load1 161 to 129), with `--basetemp` under the lane's scratch directory. All 27 failed.
- `test_frozen_validate_checkout_recovers_each_durable_crash_boundary`: 9 parameters (the `after-ownerless-validate-*` boundaries).
- `test_result_recorded_frozen_removal_resumes_from_its_journal`: the same 9 boundaries.
- 9 others:
  - `test_release_frozen_incomplete_resumes_from_its_journal` x2;
  - `test_frozen_validate_recovery_refuses_exclusion_identity_tampering` x2;
  - `test_frozen_validate_batch_accepts_exact_validation_removal_proof`;
  - `test_frozen_validate_batch_closes_operation_owned_fds_before_censuses`;
  - `test_ownerless_validate_batch_removes_terminal_frozen_checkout`;
  - `test_ownerless_validate_recovery_resumes_without_reauthorizing`;
  - `test_recover_ownerless_validate_cargo_home_requires_terminal_or_manual_evidence`.

The leading attribution is the test environment. The ownerless/frozen-validate exclusion needs a trusted, root-owned, sticky (01777) parent on the same filesystem as the fenced path. The run's log shows the refusal 12 times: "validation exclusion found no trusted root-owned, same-filesystem sticky parent for <scratch path>". A lane's scratch temp root under its home directory has no such parent. The gate's wrkslots steps run under `with-private-tmp --host-temp`, a private root under the host temp dir, whose root-owned sticky parent is `/tmp` (#2383).

Not every failure shows that refusal. Some fail on a different assertion:
- `test_frozen_validate_batch_accepts_exact_validation_removal_proof`: the batch removed nothing;
- `test_ownerless_validate_batch_removes_terminal_frozen_checkout`: a filesystem blocker;
- `test_frozen_validate_batch_closes_operation_owned_fds_before_censuses`: the closed-fds record is missing;
- `test_ownerless_validate_recovery_resumes_without_reauthorizing`: 3 against 86.
So the attribution must be shown per case, not assumed. `test_frozen_validate_batch_closes_operation_owned_fds_before_censuses` is a candidate REAL defect until evidence shows otherwise.

# Design

- Evidence per case, one table: the outcome inside the gate's own wrapper (`with-private-tmp --host-temp`, the lifecycle shard's namespace or host mode as its marks say) against the outcome from a lane scratch root. Runs are alone and repeated, with load1 recorded.
- A case that passes in the gate's environment and fails only for the missing sticky parent is environmental. Name it so in the test, and make it run where its environment exists: skip with a named reason when no trusted sticky parent is available, or give it a fixture parent. It must never fail silently, and never be skipped inside a gate.
- A case that also fails in the gate's environment, or for a reason other than the environment, is a real defect. Name its cause and fix it. Start with `test_frozen_validate_batch_closes_operation_owned_fds_before_censuses`.
- No production change for the environmental cases. No wall-clock budget (owner rule 346).

# Acceptance Criteria

- A per-case table (27 rows): outcome in the gate's wrapper, outcome from a lane scratch root, load1, and the named cause.
- Each environmental case states its requirement and runs where it is met; outside it, the case skips with that requirement named, or runs on a fixture.
- Each real defect has a named cause and a fix (or its own issue), starting with the closed-fds case.
- The lifecycle shards stay green in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned).

Evidence: `cl-census`'s receipts for this run (the failed list, the run log and the command line), kept by that lane while this issue is open.

Related:
- #2383: the gate's private temp root and the wrkslots host-temp exception (#2415 interim).
- #2065: the hot-host census tests.
