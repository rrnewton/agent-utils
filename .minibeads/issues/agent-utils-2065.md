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
updated_at: 2026-11-27T18:50:00-05:00
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

[watermelon-full-preview] Step 1 landed as 3d26c0aa on the default branch (verified-overlap landing under a coordinator waiver: fresh gate 107 of 111 steps passed, none of the 31 quarantined cases ran, remaining reds were rotating census victims in untouched differentials). Step 2 (fresh lane) is the hermetic return: drive every quarantined case above from a controlled inventory — scripted process snapshots in Python, the test-only fake process table for the holder scan in Rust, snapshot/injected evidence for the Rust differentials — covering census-not-closed, exiting process or thread with an unreadable fd directory, and unrelated non-dumpable same-UID process deterministically, and remove each mark as its family passes under gate load. Step 2 scope also covers the wider differential surface the step-1 gates exposed as rotating victims of the same cause (remove and audit Rust differentials whose verdicts read the live table, for example the ignored-file and unrelated-pane remove differentials and audit planning exhaustion): inventory that surface first, hermetically cover or quarantine each case found, and do not chase victims one gate at a time. The fail-closed rule for a process that provably is not a holder is decided and documented in step 2; any production census-behaviour change is a separate cold-reviewed commit. Timing-budget cases are tracked separately under #2031, and CPU-cap sizing under #2084.

2026-11-27, seen while gating #2388 and #2340. Four gating cases that are not in the quarantine failed with the same remove-batch refusal, each on a different tree: "REFUSED: this remove-agent-batch item used its 75-second budget for work before deletion at the /proc scan after the path fence". The cases on 668e988f and ba707504 passed in their exclusive reruns; I did not check reruns for the other two.

- `test_shared_submodules.py::test_remove_keeps_every_linked_worktree_index_of_private_clone`: 252bf52d, shard-15.
- `test_shared_submodules.py::test_remove_never_loses_index_only_blob_of_private_clone`: 668e988f, shard-0.
- `test_audit_state_rust.py::test_audit_validate_nested_repo_disposable_matches_python` and `test_audit_validate_relative_run_handle_matches_python`: 2065ee89, shard-15.
- `test_shared_submodules.py::test_remove_accepts_shared_placed_submodule_in_flat_layout`: ba707504, shard-6.

The holder scan's 600 s timeouts in the two image unmount tests are the #2372 sub-cause, made hermetic by 906b54fc.

[Claude Opus 5.5, cl-flaketriage] Step A' (owner rule 346; coordinator-approved method, 2026-11-27): the non-lifecycle wrkslots shards no longer read the host process table.

- **Change.**
  - The sixteen `python.wrkslots.shard-*` steps now run inside the lifecycle shards' unprivileged wrapper: `unshare --user --map-root-user --pid --fork --mount-proc` plus `scripts/pid_namespace_init.py`. No sudo and no root mounts are involved. A removal census there reads only the shard's own processes.
  - The shards deselect `ordinary_environment`. Those cases run on the host in `python.wrkslots.host-0`, `host-1` and `host-2`, under the `host-lifecycle` cap.
  - The `python.wrkslots.shard-*` known-red wildcard is retired, so wrkslots changes get wrkslots signal again.
  - Contract tests pin the wrapper and the partition.
- **Why.**
  - Measured 2026-11-27 at load1 190-205: one post-fence census over 6,300 host processes (115,600 threads, 284,000 fds, 1.34 GB of mountinfo) took about 30 CPU-s.
  - The remove item budget is 60/75 s of wall time. Remove and retire tests therefore refused on unrelated heads: `REFUSED: this remove-agent-batch item used its 75-second budget ... at the /proc scan after the path fence`.
  - In the discovery gate (2065ee89, `1795811375516954846-8b13c4c6`) the namespaced shards ran in 644-1135 s, against 1400-2600 s on the host, with 2,513 passed.
- **`ordinary_environment` inventory.** These need host identity; each mark carries a one-line reason.
  - Module-level:
    - `test_validation_run_liveness.py` (98): validation-run evidence needs the host's initial PID namespace.
    - `test_tool_authority.py` (5, parametrized): tool authority trusts only root-owned executables, which a user namespace maps to nobody.
    - `test_recover_parent_death.py` (3): host-visible process IDs.
    - `test_absent_slot_closure.py` (7) and `test_validate_teardown_closure.py` (4): they skip outside the initial identity user namespace, so they would have been lost silently.
  - Per test:
    - `test_audit_state_rust.py` (15): validation-run handles are judged against host PIDs and user-systemd units.
    - `test_census_threads.py` (7) and `test_census_late_fork.py` (2): the real census commands are trusted only in the initial identity user namespace.
    - `test_sandbox_root_spec.py` (5): the real uid and passwd home.
    - `test_cli.py` (4): root-owned git and real scope masks.
    - `test_remove_write_rust.py` (4): the Rust census names the test's own child as a holder inside the namespace.
    - `test_classify_journals_rust.py` (3), `test_recover_abort_own.py` (1) and `test_eden_restart.py` (1): host-visible PIDs.
    - `test_slice_fallbacks.py` (2): root-owned sudo and host scopes.
    - `test_seal_recover_scoped_census.py` (2) and `test_recover_late_census_rollback.py` (1): the real census.
    - `test_release_frozen_rust.py` (1): a root-owned sticky /tmp parent.
  - In all, 282 cases run on the host (104, 83 and 95 per host shard).
- **`load_quarantine` additions.** Each has a #2065 pointer and stays runnable with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`.
  - Host cases that need a live census inside the remove budget:
    - `test_remove_write_rust.py`: 4 cases;
    - `test_seal_recover_scoped_census.py`: 2 cases;
    - `test_recover_late_census_rollback.py::test_early_census_refusal_with_fenced_holder_retains_journal_and_retry_removes`;
    - `test_slice_fallbacks.py::test_teardown_without_a_slice_falls_back_to_the_census`;
    - `test_retire_lock_scope.py::test_retire_pending_removes_without_candidate_validation_under_lock`. Its write-handoff checks the owner's host PID; it was red on 50a174b4's gate.
  - Load, not namespace:
    - `test_shared_submodules.py::test_remove_never_loses_index_only_blob_of_private_clone`: the item budget runs out in salvage git work even in the namespace.
    - `test_census_late_fork.py::test_naming_matches_read_argv_and_environ_from_proc`: red on unrelated heads.
    - `test_lifecycle.py::test_validate_batch_census_deadline_restores_seals_before_outer_bound`. It asserts an 8 s wall-clock rollback reserve, measured at 10.18 s on gate `1795786086386213333-9d85f162`. Proposed fix: assert ordering and outcome, or a budget derived from measured CPU.
- **Not un-quarantined in this commit.** Quarantined cases are deselected in gates, so none passed in this gate, as the coordinator's rule requires.
  - Local evidence for a later step: 12 of the 21 existing #2065 Python cases passed inside the namespace on 2026-11-27. They are `test_batch_lock_scope` x2, `test_dead_fuse_entry` x2, `test_remove_fenced_lock_scope` x6, `test_journal_actor_liveness::test_remove_waits_for_a_live_finish_journal_then_proceeds` and `test_audit_state_rust::test_audit_size_unknown_text_matches_python`.
  - `test_wait_lock_waits_for_a_live_journal_then_proceeds` still failed: a 300 s wall-clock thread join.
- **Coverage.** Removal-path coverage for the quarantined set now rests on #2392's new tests and on this on-demand set.
- **Related.**
  - Two `test_worktrees_image` busy-holder tests hit the 600 s per-test timeout inside `scan_mount_holders` on another gate. #2372's test fix (906b54fc) gives them a fixture proc root, and its production change (#2372 50a174b4, pending) parses each distinct mount table once.
  - Outside #2065, noted for #2382's audit annex: `cross.herdr-run.differential`, `rust.merge-queue.test` and `vibe-talk.check.documents-browser` were red in the discovery gate.
