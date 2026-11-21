---
title: 'hot-host-census-tests: wrkslots census and agentctl revive tests fail on a busy host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- agentctl
- testing
created_at: 2026-11-21T12:45:00-05:00
updated_at: 2026-11-21T18:14:40-05:00
---

# Description

On 2026-11-21 every full gate of a tree touching wrkslots or agentctl went red in steps its change does not touch, on different shards each time, because the tests depend on the real host process table:

- same-UID census that never closes while unrelated processes fork and exit (see #1957);
- Rust/Python census differentials that disagree when an exiting process has an unreadable fd directory (Rust says indeterminate, Python says deletable);
- agentctl revive and dead-adoption tests that fail closed when any unrelated same-UID non-dumpable process exists on the host during the run;
- shards that hit their CPU limit under load.

Each named failure passes when run alone. The owner's rule is that every gating check passes on a host above 75% CPU and nothing time- or host-churn-sensitive gates.

# Design

Fast first: quarantine the affected cases out of gating, each with a pointer to this issue. Then make them hermetic: drive the census and the holder scan from a controlled process inventory (a fake proc root or an injected inventory) instead of the live host table, so the tests check the logic deterministically. Keep at most one opt-in real-host smoke test outside the gate.

# Acceptance Criteria

- No gating wrkslots or agentctl test reads the live host process table for its verdict.
- The census-not-closed, exiting-process and non-dumpable-holder cases are each covered by deterministic tests on a controlled inventory.
- The quarantine is removed once the hermetic versions pass under the gate's load.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

[watermelon-full-preview] Step 1 (quarantine, owner rule 346; no production code changed). The whole live-census set in the affected files is taken out until step 2 makes it hermetic: every case whose verdict needs a real `/proc` census to complete (or to refuse for a live holder) inside its operation budget. Each carries a pointer to this issue and stays runnable on demand. Cases that refuse before any census (owner not proven dead, nonempty slice, busy journal, dead-entry finding), pure journal compares, synthetic-cgroup reads, and cases with stubbed censuses keep gating.

- Python `load_quarantine` (run with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`), 22 cases: `test_recover_late_census_rollback.py::test_real_holder_late_census_refusal_in_recover_unfences` (see #1957; the injected-refusal sibling still gates); `test_audit_state_rust.py::test_audit_size_unknown_text_matches_python`; in `test_absent_slot_closure.py`: `test_reparented_descendant_holding_a_deleted_fd_does_not_block`, `test_a_live_reference_to_an_existing_slot_still_refuses`, `test_recover_absent_sealed_target_with_a_deleted_fd_descendant`, `test_recover_refuses_a_spoofed_deleted_directory_name`, `test_a_spoofed_deleted_sibling_directory_still_refuses`, `test_a_linked_fd_with_the_deleted_roots_text_still_refuses`; both `test_batch_lock_scope.py` removes; `test_dead_fuse_entry.py::test_create_proceeds_with_a_finding_on_another_slots_dead_entry` and `test_remove_proceeds_for_another_slot_with_a_dead_entry`; `test_journal_actor_liveness.py::test_wait_lock_waits_for_a_live_journal_then_proceeds` and `test_remove_waits_for_a_live_finish_journal_then_proceeds`; in `test_remove_fenced_lock_scope.py`: `test_coordinator_remover_blocked_mid_deletion_refuses_child_recover`, `test_deferred_remove_with_sidecarred_refs_completes`, `test_coordinator_remover_paused_before_completion_refuses_child_recover`, `test_grouped_member_paused_in_group_scan_refuses_child_recover`, `test_refused_recover_leaves_sealed_fence_and_state_unchanged`, `test_grouped_batch_members_still_defer_their_deletion`; both live-scan `test_validate_teardown_closure.py` cases. Observed verdicts under churn: census did-not-close, operation-wide/privileged census time-bound expiry, the 60-second pre-deletion scan budget, and wall-clock waits for a worker that must traverse those censuses.
- Rust `#[ignore]` (run with `cargo test -- --ignored <name>`), 9 cases: `validation_recovery::tests::checkout_facts_match_python` and `frozen_disposition_historical_schema2_matches_python` (exiting process/thread with an unreadable fd directory: Rust indeterminate where Python answers), and seven previous-boot revive / dead-native cases whose live `/proc` holder scan fails closed on any unrelated same-UID non-dumpable harness process: `dead_native_live_session_holder_refuses`, `revive_previous_boot_missing_pane_with_stale_agent_revives`, `revive_candidate_holding_session_after_launch_is_not_refused`, `revive_archived_completes_while_this_process_already_holds_the_queue_locks`, `a_muse_lane_records_the_session_its_runtime_holds_and_revives_from_the_archive_without_a_bind`, `a_codex_lane_records_its_main_thread_rollout_and_revives_from_the_archive`, `a_claude_lane_carries_the_id_it_was_launched_with_and_revives_from_the_archive`. Five further cases from the same family (`revive_live_session_holder_refuses`, `revive_dry_run_with_a_gone_pane_never_proves_unrelated_shells`, `revive_heals_a_slot_project_recorded_as_the_slot_itself`, `five_stop_and_revive_cycles_complete_for_every_harness_that_revives`, `dead_native_retirement_previous_boot_missing_pane_stale_agent_archives`) were made hermetic by #2037's test-only fake process table and keep gating; step 2 reuses that seam for the seven above.

Shard CPU timeouts (family (d)): the sixteen wrkslots shards are already balanced by recorded solo durations (about 114 s each of about 1825 s over 814 weighted families), yet a shard burned past its 2400 CPU-second cap at about three quarters through 147 cases. That is per-case census CPU on a large, churning process table (each differential scans in both editions and churn multiplies inventories), not an unbalanced split, so raising `cpu_timeout` would only license more live-table scanning. No shard or budget change in step 1: step 2 cuts the CPU by removing the live scans, then re-measure and only consider a finer split (with regenerated weights) if a shard still sits near the cap.

Step 2 then drives each census from a controlled inventory (fake proc root or injected inventory, whichever each edition already half-supports), covering census-not-closed, exiting-process-with-unreadable-fd, and unrelated non-dumpable same-UID process deterministically in both editions, and removes each quarantine mark as its case passes under gate load. What the fail-closed holder rule should be for a process that provably is not a holder is decided and documented in step 2; any production behaviour change there is a separate, cold-reviewed commit.
