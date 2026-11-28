---
title: 'lifecycle-recovery-cases-outside-gates: 27 test_lifecycle.py frozen/ownerless-validate recovery cases fail outside a gate; establish per case whether the environment or the code is at fault'
status: closed
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- lifecycle
created_at: 2026-11-28T08:41:00-05:00
updated_at: 2026-11-28T12:55:00-05:00
closed_at: 2026-11-28T12:55:00-05:00
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

Evidence (2026-11-28, `cl-flaketriage`, main 3192c821). The 27 ids were run alone, 3 rounds each:

| environment | ids | result | load1 at start |
|---|---|---|---|
| gate wrapper, namespace mode (`with-private-tmp --host-temp` + user/PID namespace; the ids not marked `ordinary_environment`) | 2 | 2 passed, 3 of 3 rounds | 218, 228, 173 |
| gate wrapper, host mode (`with-private-tmp --host-temp`; the `ordinary_environment` ids) | 25 | 25 passed, 3 of 3 rounds | 238, 233, 172 |
| lane scratch root (plain pytest, `--basetemp` under the lane's scratch directory) | 27 | 27 FAILED, 3 of 3 rounds | 234, 246, 164 |

Per case (rows grouped by test function):

| case | ids | gate wrapper (mode) | lane scratch root | cause in the lane run |
|---|---|---|---|---|
| `test_result_recorded_frozen_removal_resumes_from_its_journal` | 9 (after-ownerless-validate-*) | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_frozen_validate_checkout_recovers_each_durable_crash_boundary` | 9 (after-ownerless-validate-*) | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_release_frozen_incomplete_resumes_from_its_journal` | 2 (after-ownerless-validate-journal, -remove) | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_frozen_validate_recovery_refuses_exclusion_identity_tampering` | 2 (guard-identity, source-alias) | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_ownerless_validate_recovery_resumes_without_reauthorizing` | 1 | pass 3 of 3 (namespace) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_recover_ownerless_validate_cargo_home_requires_terminal_or_manual_evidence` | 1 | pass 3 of 3 (namespace) | FAIL 3 of 3 | the sticky-parent requirement, raised |
| `test_frozen_validate_batch_accepts_exact_validation_removal_proof` | 1 | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, the batch retained the checkout with it as the reason |
| `test_ownerless_validate_batch_removes_terminal_frozen_checkout` | 1 | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, the batch retained the checkout with it as the reason |
| `test_frozen_validate_batch_closes_operation_owned_fds_before_censuses` | 1 | pass 3 of 3 (host) | FAIL 3 of 3 | the sticky-parent requirement, the batch retained the checkout with it as the reason, so the fresh census never ran (census stages: shared only) |

Named cause, the same for all 27: "validation exclusion found no trusted root-owned, same-filesystem sticky parent". The lane scratch root is not in the filesystem view of the host temp dir ("validation path is outside the filesystem view containing the exclusion root").
- The candidate real defect, the closed-fds case, is environmental too. Its batch retains the checkout for this reason before the exclusion phase, so the closed-fds record is never written. This was captured from the batch report.
- No production change.

Fix (test-only): one guard, `require_host_temp_exclusion_view`, at the start of each of the 27 cases. For the two parametrized functions it applies only to the ownerless-validate boundaries; their tampered-* parameters do not reach the exclusion and still run anywhere.
- The guard runs the product's own view check (`_mount_alias_path`) from the test temp dir to the host temp dir.
- Outside a gate, a missing view SKIPS the case, naming the requirement and the remedy (`scripts/with-private-tmp --host-temp`).
- Inside a gate (`GATE_ADMISSION_RUN_ID` set) the guard never skips; it FAILS.
- `test_exclusion_view_guard_fails_rather_than_skips_inside_a_gate` proves that branch: with the view check forced to refuse and the gate marker set, a skip fails the test (the lead's condition).

Related:
- #2383: the gate's private temp root and the wrkslots host-temp exception (#2415 interim).
- #2065: the hot-host census tests.

## Resolution

Fixed in 82f6d6f2 (`py/wrkslots/tests/test_lifecycle.py` only, test-only; the evidence notes in 71fe10ca), gated as 7582169c.
- Gate 1795883841170273936-02c92bc8 (full selection): all 9 lifecycle steps passed with 0 failed. The 27 ran in the gate's own shards, located by re-collecting each shard with the gate's command (every shard digest matched the gate log):
  - 25 in host mode (host shards 0, 1, 2: 21, 3 and 1 cases);
  - 2 in namespace mode (namespace shards 2 and 4);
  - the guard test in namespace shard 0.
- The guard cannot skip inside a gate (`GATE_ADMISSION_RUN_ID` is set in every gate step), so all 27 passed and none skipped.
- The 5 skips in those shards are the pre-existing root-coverage skips: `test_lifecycle.py:1188` x3, `foreign_mount_ns_cases.py:631`, `test_lifecycle.py:28896`. They were named by a re-run of the three shards with `-rs`, at the same counts.
- Off-path reds in that gate were cleared by a solo rerun on the unchanged commit (gate 1795886405429317163-896d240e, rerun passed): `python.wrkslots.shard-0` (#2481), `python.agentctl.shard-0` (#2482), `rust.merge-queue.test` (#2458). The two vibe-talk known-red steps were untouched.
- Landed with the coordinator's overlap exception: #2468 also changed this file, in disjoint hunks. On the combined tree the 27 cases, the 2 tampered parameters, the guard test and #2468's two tests passed, and strict mypy was clean.
- No real defect was found among the 27, the closed-fds case included. No production change.

