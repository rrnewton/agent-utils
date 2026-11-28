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
updated_at: 2026-11-28T13:08:00-05:00
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
    - `test_audit_state_rust.py` (2 more, back in the namespaced shards since the git stand-in note below): `test_audit_planning_fair_share_matches_python` and `test_audit_census_finalize_reserve_matches_python`. A host git wrapper's detached helper outlives the audit's own git calls with its working directory in the slot, and the fast census in a PID namespace counts it as a running agent. See the rebased-gate note below.
    - `test_census_threads.py` (7) and `test_census_late_fork.py` (2): the real census commands are trusted only in the initial identity user namespace.
    - `test_sandbox_root_spec.py` (5): the real uid and passwd home.
    - `test_cli.py` (4): root-owned git and real scope masks.
    - `test_remove_write_rust.py` (9, back in the namespaced shards since the git stand-in note below): a host git wrapper's detached helper outlives wrkslotsd's own git calls with its working directory in the slot, and the fast census in a PID namespace names it as a holder. See the final-gate note below.
    - `test_classify_journals_rust.py` (3), `test_recover_abort_own.py` (1) and `test_eden_restart.py` (1): host-visible PIDs.
    - `test_slice_fallbacks.py` (2): root-owned sudo and host scopes.
    - `test_seal_recover_scoped_census.py` (2) and `test_recover_late_census_rollback.py` (1): the real census.
    - `test_release_frozen_rust.py` (1): a root-owned sticky /tmp parent.
  - In all, 283 cases ran on the host (81, 96 and 106 per host shard), the 9 `test_remove_write_rust.py` cases only on opt-in (`load_quarantine`). With the git stand-in, all 11 helper cases leave the host shards and run in the namespaced ones: 281 cases now run on the host (95, 88 and 98 per host shard; the 9 remove cases had run there only on opt-in).
- **`load_quarantine` additions.** Each has a #2065 pointer and stays runnable with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`.
  - Host cases that need a live census inside the remove budget:
    - `test_remove_write_rust.py`: 9 cases (un-quarantined with the git stand-in; see its note);
    - `test_seal_recover_scoped_census.py`: 2 cases;
    - `test_recover_late_census_rollback.py::test_early_census_refusal_with_fenced_holder_retains_journal_and_retry_removes`;
    - `test_slice_fallbacks.py::test_teardown_without_a_slice_falls_back_to_the_census`;
    - `test_retire_lock_scope.py::test_retire_pending_removes_without_candidate_validation_under_lock`. Its write-handoff checks the owner's host PID; it was red on 50a174b4's gate.
  - Load, not namespace:
    - `test_shared_submodules.py::test_remove_never_loses_index_only_blob_of_private_clone`: the item budget runs out in salvage git work even in the namespace.
    - `test_census_late_fork.py::test_naming_matches_read_argv_and_environ_from_proc`: red on unrelated heads.
    - `test_lifecycle.py::test_validate_batch_census_deadline_restores_seals_before_outer_bound`. It asserts an 8 s wall-clock rollback reserve, measured at 10.18 s on gate `1795786086386213333-9d85f162`. Proposed fix: assert ordering and outcome, or a budget derived from measured CPU. It was red on more than 27 heads on 2026-11-27 (29.6 s and 11.3 s among them).
    - `test_worktrees_image.py::test_unmount_busy_clears_on_a_bounded_retry`: it times out in the real host /proc holder scan, as #2372's two image tests did. The fixture proc root for it is #2434.
    - `test_shared_submodules.py::test_remove_keeps_every_linked_worktree_index_of_private_clone`: the remove item budget ran out in salvage git work even in the namespace, like its quarantined sibling (gate `1795844957763037888-56f2b0d5`).
  - Host, on the live census:
    - `test_release_frozen_rust.py::test_release_yes_matches_python`: the host /proc census met an unrelated exiting thread whose fd directory was unreadable (case 2 above), on the same gate. It stays `ordinary_environment` for its root-owned sticky /tmp.
- **Not un-quarantined in this commit.** Quarantined cases are deselected in gates, so none passed in this gate, as the coordinator's rule requires.
  - Local evidence for a later step: 12 of the 21 existing #2065 Python cases passed inside the namespace on 2026-11-27. They are `test_batch_lock_scope` x2, `test_dead_fuse_entry` x2, `test_remove_fenced_lock_scope` x6, `test_journal_actor_liveness::test_remove_waits_for_a_live_finish_journal_then_proceeds` and `test_audit_state_rust::test_audit_size_unknown_text_matches_python`.
  - `test_wait_lock_waits_for_a_live_journal_then_proceeds` still failed: a 300 s wall-clock thread join.
- **Final gate (aa943857, `1795818591782702449-bc4ab3a5`).** Two more `test_remove_write_rust.py` cases refused in the namespace: `live process N uses slot ... (working directory)`.
  - The holder was identified with a sampler of every process whose working directory was in a test slot. In all 9 refusals seen (2 in the gate, 7 in local namespace runs) it was the same thing: a detached helper that the host's git wrapper leaves after a git call, running `git remote get-url origin` in the checkout, orphaned to the namespace init.
  - Each Rust remove's own git calls leave about 30-65 such helpers in its slot. The Rust post-fence census runs right after those calls and, among few processes, reaches one while it lives. The Python census walks the fenced tree first, and the host census is slow, so both usually miss it.
  - The discovery note's cause ("the census names the test's own child") was an inference. It is replaced by this named cause, and the mark reasons say so.
  - Five more cases get the same pair of marks: `test_remove_with_cache_globs_matches_python`, `test_remove_ignored_cache_target_still_removes_in_both_editions`, `test_remove_event_log_replays_through_rust_reader`, `test_remove_writes_the_same_finish_journal_as_python` and `test_remove_ignored_file_discards_in_both_editions`. The last one has not refused yet but leaves the same helpers in its slot.
  - The product side is #2418: wrkslots waits for its own commands' descendants before a census.
- **Rebased gate (91d33254, `1795824135544394071-a651fb9e`).** Two audit differentials disagreed between editions in the namespaced shards: Rust counted a running agent that Python did not.
  - Five runs alone in the namespace with the sampler all passed, but every run had about 130 of the host git wrapper's orphaned helpers working in the slot the audit judges.
  - Both editions count a live process using the slot as a running agent, so the faster Rust census seeing one is the #2418 cause, not an edition bug.
  - The two tests are `ordinary_environment` with that reason. Their 15 sibling audit differentials already pass on the host shards.
- **Test fix, not a mark.** `test_eden_restart.py::test_hung_daemon_times_out_and_the_other_slot_still_restarts` patched both eden timeouts to 0.5 s, so the healthy slot's start was refused inside a gate's host shard. Both timeouts are now 10 s: the hung fake sleeps 30 s and still times out by name. Alone 5 of 5 at load1 215-261.
- **Landed (2026-11-28).** Step A', its follow-ups and option B' as f58a74e0 (8d6c7421..f58a74e0, seven commits). The gate on the source head 54b0854a (1795865238081247654-6865f313) had all 30 wrkslots and lifecycle steps green. 28 sessions reported 94,442 git calls in their own namespace, none fell back. Four off-path reds were waived (liaison 1171): `rust.agentctl.test` (#2381, console refused under the lane's harness), `rust.merge-queue.test` (CPU cap), `vibe-talk.check.discuss-browser` (#2175 known-red) and `vibe-talk.check.thread-reader-resume-browser`.
  - Still open against the acceptance criteria: the 281 host-identity cases read the live host process table in the host shards, and the load quarantines above remain until hermetic versions exist (the fixture proc root of #2434, a controlled inventory for the census cases). The product side of the helper race is #2418.
- **C1, out of quarantine (2026-11-28).** Fifteen namespaced cases leave the load quarantine. Each was quarantined for the host census or host churn, which the namespaced shards remove. Evidence: inside the gate's own wrapper (`with-private-tmp --host-temp` plus the user/PID namespace), quarantine opted in, with the session git stand-in, 18 cases passed 5 of 5 rounds at load1 141-187 (about 7,550 git calls a round, none fell back).
  - 13 by mark only: `test_batch_lock_scope` x2, `test_dead_fuse_entry` x2, `test_journal_actor_liveness::test_remove_waits_for_a_live_finish_journal_then_proceeds`, `test_remove_fenced_lock_scope` x6, `test_census_late_fork::test_naming_matches_read_argv_and_environ_from_proc`, `test_audit_state_rust::test_audit_size_unknown_text_matches_python`.
  - 2 with a test fix (owner rule 346): `test_lifecycle.py::test_validate_batch_census_deadline_restores_seals_before_outer_bound` now checks census-call order and the outcome instead of an 8 s wall-clock rollback reserve. `test_journal_actor_liveness::test_wait_lock_waits_for_a_live_journal_then_proceeds` waits for the adopt's verdict on an event, with a 900 s hang backstop past the adopt's own 300 s `--wait-lock` bound, instead of a 300 s join.
  - Held, pending the coordinator: `test_shared_submodules.py` x2. They passed 5 of 5 too, but their cause is the product's per-item remove budget (below) spent in salvage git work under load, not the census.
  - Still quarantined: the 14 host-shard cases that need the initial user namespace, and `test_worktrees_image.py::test_unmount_busy_clears_on_a_bounded_retry` (#2434).
- **The "60 s budget" in the quarantine reasons is a product budget.** `_AGENT_REMOVE_BATCH_ITEM_SECONDS = 60.0` (`py/wrkslots/cli.py`, next to its rationale) bounds how long one remove-agent-batch item holds the registry lock before its first deletion. Other clients wait for that lock with finite bounds (a heartbeat waits 110 s). Ordinary remove of an agent slot runs as a batch of one, so it has the budget too.
  - It grows 15 s per nested submodule, up to 600 s. Each /proc scan inside it gets 30 s plus 10 s per 1,000 processes, held to what is left of the item budget.
  - When it runs out, the item is refused ("this remove-agent-batch item used its N-second budget for work ...") and the slot is kept.
  - So on a busy host a real remove can refuse here: a host-wide census at load about 250 is what spends it. It fails safe, but it is a wall-clock product bound. Whether it should count CPU or retry is a product question, not a test one.
- **Coverage.** Removal-path coverage for the quarantined set now rests on #2392's new tests and on this on-demand set. The 9 `test_remove_write_rust.py` Rust/Python remove differentials are back in the namespaced shards with the git stand-in (next note); #2418 stays the product fix.
- **Git stand-in, option B' (2026-11-28, coordinator's approval).** The wrkslots tests put a git stand-in first on PATH for the session (`py/wrkslots/tests/git_stand_in.c` and `git_stand_in.py`, wired in the conftest).
  - The stand-in runs the real git in its own unprivileged user and PID namespace under a small init. When git exits, the kernel kills whatever git left behind (the host wrapper's helper) before the call returns. No privilege beyond unprivileged user namespaces; plain git where they are refused, counted in the session summary.
  - It is compiled once per session into a private directory under `TMPDIR` (a gate's private run root), or the checkout's ignored `.pytest_cache` when `TMPDIR` is unset or the host temp dir. The directory is removed at session end.
  - Off (no C compiler, or namespaces refused by a probe call at session start): the stand-in stays off PATH, the summary says why, and cases marked `git_stand_in` are quarantined for that session, so their helper reds cannot come back.
  - Marks: the 11 `ordinary_environment` helper marks (remove x9, audit x2) and the remove x9 `load_quarantine` are gone; those 11 and the three later helper victims (`test_remove_lock_scope_rust::test_remove_waits_for_a_live_finish_journal_then_proceeds`, `test_validate_nested_git_policy::test_audit_and_validation_removal_agree_on_one_tree`, `test_audit_lock_free::test_other_edition_pools_rows_with_the_same_report`) are marked `git_stand_in`.
  - Measured (namespaced shards, alternating, under a disk floor), direct git -> stand-in: shard 0 195 s / 646 CPU-s -> 204 s / 538; shard 1 302 s / 413 -> 328 s / 275; shard 15 183 s / 322 -> 210 s / 196 (load1 118-145). Per git call 19.4 ms -> 21.0 ms median. Orphaned helpers seen 40/45 and 254/265 -> 0. 4,489, 6,285 and 5,013 calls, none fell back. CPU falls because the helpers no longer run.
  - The 14 marked cases in the namespace, 3 rounds alternating: without the stand-in 3, 6 and 4 of 14 failed (helper reds); with it 14 of 14 each round, no fallback.
  - Every other wrkslots step ran with it (namespaced shards 2-14, host shards 0-2, lifecycle namespace 0-5 and host 0-2). Each failure was rerun without the stand-in: all fail the same way without it (local temp-dir artifacts), and the few that differed between the two runner invocations pass both ways when run identically, 2 rounds each. No test relied on a git child outliving its call.
  - Earlier prototypes, dropped: a subreaper that waits for every descendant (shards 2.4-2.75x slower: each call waits out the helper) and the namespace stand-in in Python (45 ms per call).
  - The stand-in's own test (`test_git_stand_in.py`) runs it for real: a detached grandchild is gone when the call returns, arguments and signal deaths pass through, and git's own SIGTERM handling decides the exit.
- **Cross-gate /dev/shm collision, fixed (2026-11-28, 6c0f7500).** The namespacing above made every namespaced shard's pytest the same small pid inside its namespace (20 in a gate). Two fixtures named their /dev/shm directory by pid: `test_remove_write_rust.py` `_own_fs` (also used by `test_remove_lock_scope_rust.py`) and `test_census_foreign_fds.py`. /dev/shm is host-wide, so the same test in two concurrent gates shared one directory, and one run's cleanup deleted the other's fixture. Two gates failed `test_remove_ignored_file_discards_in_both_editions` that way in the same minutes. Both sites now use `tempfile.mkdtemp(dir="/dev/shm")`. Control: two namespaced runs of the case at once, 3 trials: main 6 of 6 failed, the fix 6 of 6 passed.
  - Sweep done for tests AND product: pid-keyed or otherwise non-unique host-wide paths (/dev/shm, /run/user, /tmp and the private-root parent /tmp/aut-<uid>, lock files, sockets, user systemd units) that a namespaced shard could share with another gate. Only the wrkslots steps run namespaced. Found safe: private_tmp.py roots (owner record with pid_namespace), the stand-in's mkdtemp, the lifecycle exclusion roots and guards, abstract sockets that embed a private path, pid-named files inside private dirs, and systemd-unit names (the user manager is unreachable from the namespace).
  - What remains:
    - #2471 socket-selftest-cross-namespace-sweep (filed): the socket-probe self-test's stale sweep judges each creator pid in its own PID namespace, under the host /tmp.
    - To audit, listed in #2471: the product readers of the real gate admission directory (`_gate_admission_dir`).
  - Found while stopping an invalid gate: #2472 gate-abort-leaves-dagrun-scope (filed). A gate stopped by SIGTERM left its dagrun scope and steps running.
- **48-hour red triage (2026-11-28), wrkslots-side items without a fix yet.** The step logs had rotated out; each item was read from the console gate log that the lane running that gate kept in its scratch directory.
  - E -> #2488: `test_classify_journals_rust::test_classify_live_seal_structured_blocking_reason_matches_python`. The Rust edition called a live-actor seal "in progress" where Python called it interrupted. It recurred: head ecf2fab5 (`python.wrkslots.shard-6`, cl-autodeny's console log) and `python.wrkslots.shard-2` of another gate (cl-flaketriage's console log). Repro first.
  - J, one occurrence, no number yet: agentctl `registry_layout::tests::an_agent_whose_wrapper_holds_its_lock_is_left_and_an_identity_agent_moves`. migrate moved the agent although the test held its wrapper lock (head 1b540308, `rust.agentctl.test`, cl-flaketriage's console log for that gate). Cause not established.
  - F, unexplained: `python.wrkslots.shard-6` ended at its CPU budget ("CPU-TIMEOUT >2400s cpu" after 2,184 s) with no failing test (head 3e4b7acd, cl-svcreload's console log). Whether that tree's file set was heavier, or a case ran away, is not known.
  - Fixed or filed elsewhere: the helper-race reds before the stand-in (above); the third shared-submodules case under #2459; the long-path edition mismatch (#2485); the lock-wait test race (#2481).
- **Related.**
  - Two `test_worktrees_image` busy-holder tests hit the 600 s per-test timeout inside `scan_mount_holders` on another gate. #2372's test fix (906b54fc) gives them a fixture proc root, and its production change (#2372 50a174b4, pending) parses each distinct mount table once.
  - Outside #2065, noted for #2382's audit annex: `cross.herdr-run.differential`, `rust.merge-queue.test` and `vibe-talk.check.documents-browser` were red in the discovery gate.
