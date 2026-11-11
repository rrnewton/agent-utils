---
title: 'wrkslots-load-robust: load-sensitive lifecycle and census cases are quarantined until they wait on conditions, not clocks'
status: open
priority: 0
issue_type: task
assignee: mu-testaudit
labels:
- wrkslots
- test-robustness
created_at: 2026-11-10T22:46:59+00:00
updated_at: 2026-11-10T22:46:59+00:00
---

# Description

[muse watermelon] Owner, reply 346 (2026-11-10): "Disable those tests and have an agent work to bring them back when they're robust. Each product lane must make its tests fast, high signal, and robust to running on a hot machine (>75% cpu on 316 HW threads). ... Anything else should be disabled until we can improve it."

The `wrkslots` Python shards are the largest flake source in gating. Gate statistics for 2026-11-10 (first-try pass rate 23.7%, flake rate 56.8% (92/162), red-gate rate 75%) put every top flaky step in this suite: `python-lifecycle.host.shard-0` (5/6 later rerun-passed), `python.wrkslots.shard-1` (5/6), `python.wrkslots.shard-2` (5/6), `python-lifecycle.namespace.shard-0` (4/6), `python.wrkslots.shard-4` (4/5). The 3-day ledger agrees (for example `python.wrkslots.shard-7` 24 gate failures, 16 on heads that later passed a solo rerun). Coordinator decision: case-level quarantine only; the shards stay, because they are the only coverage for most slot behaviour. Whole-shard disable needs explicit coordinator approval.

Quarantined cases (each: failed in a gate, then passed solo, or failed on main under load with the stated mechanism). Quarantined means excluded from default shard runs, runnable by explicit opt-in, and listed here until returned:

- Ownerless lifecycle cases in `py/wrkslots/tests/test_lifecycle.py` that fail on main under load (load average about 80, 3 of 9 cases) with "same-UID process census did not close: each of 8 process inventories found generations that may still use the target". A census that must close within a fixed inventory count cannot hold on a hot host that keeps forking.
- `test_audit_state_rust::test_audit_follows_a_matched_holder_replaced_before_its_recheck` (#1102 holder churn): flaked in a gate, 3 of 3 solo.
- `test_remove_write_rust::test_remove_forced_churn_refuses_like_python` (`python.wrkslots` shard-0): failed once in a gate with the churn fixture's `appeared.pid` missing; solo rerun passed.
- `test_remove_refuses_when_a_process_uses_the_state_home` (`python.wrkslots` shard-0): under load the 60 s pre-deletion budget expires at `git rev-parse` before the state-home check it expects; failed the same way in two full gates and one solo run. A wall-clock budget asserted under load.
- Further cases with the same evidence shape (gate failure then solo pass in the last 3 days of the gate ledger) are enumerated in the test-robustness audit (harness `ai_docs/2026-11-10-test-robustness-audit.md`) and appended to Notes as they are confirmed case by case; no case is quarantined on shard-level statistics alone.

# Design

- Quarantine wiring: a pytest marker (or deselect list) honoured by `scripts/run_pytest_shard.py` and `scripts/run_component_tests.py`, so every shard excludes quarantined cases by default and an explicit opt-in includes them unchanged. Shard balance and the disjoint-and-complete sharder proof must account for quarantined cases explicitly (counted, named, never silently dropped).
- Return condition per case: waits on events or conditions (a holder appears, a generation is observed gone, a file exists) with no fixed retry count, sleep race or wall-clock budget deciding the verdict; census cases must not require the census to "close" within N inventories while the host forks. Where the product itself enforces a budget (the 60 s pre-deletion bound), the test asserts the refusal reason, not that unrelated Git work finished inside the budget on a loaded host.
- Proof bar for return: the case passes repeatedly under load on this host class (316 hardware threads, >75% CPU), with the run count and load recorded in Notes; touched-code flake rule applies (5 solo passes plus a named load cause for any earlier failure).

# Acceptance Criteria

- Every quarantined case above is excluded from default shard runs, included by the opt-in, and named in test output or plan output as quarantined under this issue.
- Shards otherwise run their full case sets; sharder completeness checks pass with quarantine accounted.
- Each returned case meets its return condition and proof bar, recorded in Notes before its quarantine mark is removed.

# Notes

Filed by the test-robustness audit lane (mu-testaudit) at owner priority. Related: #1249 cross-differentials-robust (the cross-lane equivalent), #1250 vibe-talk-timing-robust. Do not widen to whole shards without coordinator approval.
- `test_audit_human_and_gate_match_python` and the `test_audit_state_rust.py` audit differentials (from #1247 wrkslots-status-lock): run-order liveness race — the fixture owner PID is live while the Python audit runs and dead when the Rust audit runs second, so return codes diverge (UNKNOWN exit 2 vs BLOCKED exit 1) with no product difference; gate-red in shards 5/6/7, solo-green. Fix direction: pin owner state identically for both editions.
- `test_audit_heavy_churn_many_slots_matches_python` (`python.wrkslots` shard-7): failed alone with "read-only census exceeded its operation-wide time bound (bound 60.0 s, elapsed 60.1 s)" at host load ~75-128. A wall-clock census bound.
- Socket-census cases in `python-lifecycle.host.shard-0` (5 cases; from devbig030 #1267 gate): "network namespaces kept appearing after 4 rounds" / "socket-table dump has no time left" under load and netns churn; solo passed. Same census-close-under-churn mechanism.
- Gate-admission candidate (coordb, devbig030): `test_gate_admission.py::test_concurrent_admission_mixed_versions_never_exceeds_the_cap` flaked in a full gate on c1b8e1b8 and passed solo. Under owner reply 346: if timing-sensitive under load, make it event/condition-based or disable it with a tracking issue.
- Gate a1d53f8e (2026-11-10) wrkslots/lifecycle failures, solo disposition pending: `test_audit_state_rust.py::test_audit_census_sees_appearing_pid_and_tolerates_vanished_pid`, `test_census_subject_progress.py::test_subject_cursor_is_fair_with_different_root_counts_and_oversized_peer`, `test_audit_state_rust.py::test_audit_deletable_matches_python`, `test_census_binding_deadline.py::test_persisted_root_validation_does_not_start_after_deadline`, `test_reclaim_slot_state.py::test_reclaim_orphan_state_apply_refuses_under_freeze`, `test_reclaim_slot_state.py::test_reclaim_orphan_state_plan_then_apply`, `test_doctor_audit_cli_rust.py::test_audit_human_and_gate_match_python`, `test_reclaim_slot_state.py::test_remove_reclaims_state_home_and_bundles_unpushed_git`, `test_remove_write_rust.py::test_remove_forced_churn_refuses_like_python`, `test_lifecycle.py::test_validate_complete_still_refuses_live_path_use_after_owner_dies`, `test_lifecycle.py::test_remove_agent_batch_removes_disk_image_slots[8]`, `test_lifecycle.py::test_batch_census_isolated_real_file_alias_mapping_and_socket[socket]`, plus `packages.standalone.wrkslots`. Each is quarantined only on gate-red then solo-green evidence.
- Socket-census cases in `python-lifecycle.host.shard-0` (5 cases; from devbig030 #1255 gate): "network namespaces kept appearing after 4 rounds" / "socket-table dump has no time left" under load and netns churn; solo passed. Same census-close-under-churn mechanism.
- `test_remove_write_rust.py::test_remove_with_image_records_matches_python` (`python.wrkslots` shard-5, devbig030): Rust remove refused on its process census ("/proc/<pid>/fd unreadable ... PF_EXITING after 2000 ms"), a 2 s wall budget under load; solo rerun passed.
- `test_lifecycle.py::test_remove_agent_batch_refuses_a_detached_holder_of_an_image_slot_after_its_fence[directory]` (gate a1d53f8e-followup, `python-lifecycle.host` shard-2): gate-red, solo-green on the same tree.
- Quarantine implemented (mu-testaudit): `load_quarantine` pytest marker registered in `py/pyproject.toml`; `py/wrkslots/tests/conftest.py` deselects marked cases by default (before shard partitioning, so shards stay disjoint and complete) and names each in the terminal summary. Runnable on demand with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`. Contract test: `py/wrkslots/tests/test_load_quarantine.py`. Cases marked: the audit differentials and churn/census cases in `test_audit_state_rust.py`, `test_audit_human_and_gate_match_python`, `test_remove_forced_churn_refuses_like_python`, `test_remove_with_image_records_matches_python`, `test_remove_refuses_when_a_process_uses_the_state_home`, the reclaim cases, `test_subject_cursor_is_fair_with_different_root_counts_and_oversized_peer`, `test_persisted_root_validation_does_not_start_after_deadline`, and the ownerless lifecycle census cases in `test_lifecycle.py`.
- Second batch (gate-red, 17 solo-green on the same tree): `test_shell_command_json_and_text_match_python`, `test_audit_forced_churn_matches_python`, `test_audit_follows_a_leader_that_exits_after_the_listing_like_python`, `test_reclaim_orphan_rust_plan_and_apply_match_python`, `test_remove_read_sidecar_retires_it`, `test_an_older_reader_refuses_only_an_outstanding_owner_journal`, `test_socket_census_is_read_only_passive_and_runs_no_sudo`, the two `test_census_traversal_deadline.py` cases, `test_empty_roots_complete_on_the_last_available_directory_observation`, `test_cache_plan_matches_python`. Marked `load_quarantine` on the same evidence.
- `test_audit_census_partial_then_complete_matches_python`: gate-red, solo-green (6.1s). Marked `load_quarantine`.
- `test_gate_admission.py::test_a_rerun_ticket_jumps_ordinary_queued_gates`: gate-red in local validate, solo-green (with `test_concurrent_admission_mixed_versions_never_exceeds_the_cap`, 2.88s). The wrkslots conftest quarantine does not cover `py/tests`; a shared marker hook there is the follow-up.
