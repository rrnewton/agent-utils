---
title: 'agentctl-lib-tests-load-robust: agentctl lib tests fail under load at default parallelism'
status: closed
priority: 1
issue_type: task
labels:
- agentctl
- tests
- flake
created_at: 2026-11-14T00:00:00.000000000+00:00
updated_at: 2026-11-14T00:00:00.000000000+00:00
---

# Description

[muse ultra] `cargo test --manifest-path rs/agentctl/Cargo.toml --lib` at default parallelism on a hot host (>75% CPU on 316 hardware threads) fails 13-16 tests per run, with the set varying between runs; every failing test passes alone. Owner rule 346: gating tests must be fast, high-signal and robust on a hot machine, and nothing real-time-sensitive may gate. The failing set is mostly `chat_service` owner-loop ordering tests (pane reads versus typing), plus `client::process_liveness` / `real_non_shell_process`, subagents lock_rank / restore / transient recovery, `team_driver::driver_ack`, and agent bounded_drain / cancellation tests — 19 unique tests across two observed runs.

# Design

Replace wall-clock waits and ordering assumptions with event or condition waits and generous timeouts (a timeout is a hang guard, never the verdict). Fix tests, not product; a real product race found along the way is fixed with its own test and called out.

Root causes: every test in the process shares one target-lock root keyed by pane digest, and the fixtures reuse one pane, so fake-clock lock polling spins behind other tests' drains; the fake Herdr event server panicked when a starved handshake missed the client's acknowledgement deadline, killing later subscriptions; the owner-loop harness treated its limit as a CPU/wall verdict instead of a hang guard; several tests staged state changes from a poller thread after fixed 300/500 ms windows that can contain no scan at all on a loaded host; one test measured a 5 s bound with the test process's own clock; process tests pinned a child's identity before its exec landed and let a 30 s child outlive its fixture wait.

# Acceptance Criteria

- The full `cargo test --manifest-path rs/agentctl/Cargo.toml --lib` at default parallelism on a hot host passes 3 runs in a row.
- No test is disabled to get green; every formerly flaky test still asserts its original behaviour, now on a condition.
- Any real product race found is fixed with its own regression test and called out in the commit body.

# Notes

Number assigned from the originating host's block; filed in the delivering commit. Companion: #1551 agentctl-chat-launch-wallclock-tests (the Python edition of the same rule, landed separately first).

## Resolution

[muse ultra] Fixed across the chat_service owner-loop harness and fake Herdr server, the agent drain and its test fakes, client process tests, and the subagents lock-rank/restore/transient tests; no test was disabled. Tests now wait on conditions (subscription and lookup counts, route-read attempts, settled request phases, read-count-anchored fake screens, exec-landed process identity, terminal breaker state) with timeouts as hang guards only. Product races found and fixed with their own commits and regression tests (each proven to fail with its fix disabled): a cancelled drain lock wait reported Pending only before typing and PossiblySubmitted (exit 76, must-not-resend wording) after it, instead of an untyped delivery error whose classification depended on which lock was contended; the team driver, queue delivery and queue binding locks now release explicitly (HeldLock), so a child forked under a lock cannot keep it held past its holder's drop.

Fixed test names: agent::tests::a_bounded_drain_stops_at_its_item_limit_deadline_or_lock_timeout; agent::tests::cancellation_before_injection_leaves_prompt_safely_pending; agent::tests::cancellation_after_injection_quarantines_unknown_outcome; chat_service::tests::after_typing_a_prompt_the_owner_loop_reads_the_pane_though_herdr_reports_idle; chat_service::tests::the_owner_loop_keeps_running_when_a_scan_cannot_read_a_reply_route; chat_service::tests::the_owner_loop_looks_the_status_up_for_a_retry_when_no_pane_event_subscription_works; chat_service::tests::the_owner_loop_reads_the_pane_before_a_rescan_types_the_prompt_of_a_new_request; chat_service::tests::the_owner_loop_reads_the_pane_before_it_types_the_prompt_of_a_new_request; chat_service::tests::the_owner_loop_types_a_prompt_left_waiting_once_herdr_reports_the_agent_ready; chat_service::tests::a_reply_sender_whose_helper_fails_trips_the_breaker_as_the_loop_does; chat_service::tests::claude_code_is_not_asked_again_for_an_idle_time_after_it_fails_or_lists_no_session; chat_service::tests::the_owner_loop_keeps_reading_the_pane_while_the_paste_hint_hides_the_running_turn; chat_service::tests::the_owner_loop_queues_a_headless_coordinators_requests_without_herdr; client::tests::process_liveness_tracks_a_real_child_before_and_after_kill; client::tests::real_non_shell_process_cannot_be_adopted_as_a_pane_shell; subagents::tests::lock_rank::a_send_waiting_for_readiness_holds_no_lock_and_types_once_the_pane_is_idle; subagents::tests::restore::watch_unit_repair_off_skips_units_but_still_drains_queues; subagents::tests::transient_recovery_resumes_a_stalled_goal_and_never_retries_quota; team_driver::tests::driver_ack_waits_expires_hands_off_then_progresses. Found only at the first full-width acceptance attempt and also fixed: cli::tests::chat_status_names_the_lock_holder_and_waiters_for_stalled_requests; foreign::ownership::tests::spawn_owned_harness_records_then_runs_and_sweeps_leftovers; headless::wrapper::tests::an_execing_launcher_pins_the_post_exec_harness_on_start_and_wrapper_restart; subagents::tests::lock_rank::a_send_whose_pane_never_becomes_ready_gives_up_at_its_deadline_and_releases_every_lock. New regression tests: agent::tests::cancellation_behind_a_held_queue_lock_is_pending_not_a_delivery_error; agent::tests::a_queue_lock_failure_after_typing_is_possibly_submitted_not_pending; agent::tests::a_typed_prompt_whose_commit_lock_failed_is_quarantined_by_the_next_drain_without_retyping; team_driver::tests::a_released_driver_lock_is_free_though_a_descriptor_copy_survives.

Acceptance: three consecutive full `cargo test --manifest-path rs/agentctl/Cargo.toml --lib` runs at default parallelism on a hot 316-thread host each reported 2628 passed, 0 failed (final tree: 2026-11-16 16:22-16:35 ET, 3x2659 passed 0 failed under a shared measurement hold on a hot host; earlier attempts' failures are diagnosed in the fix commits); the Python companion acceptance (#1551) passed under a 16-core capped burner. Disclosure (coord7): their arm64 Buck run (remote execution, client capped at 4 cores, nice 19) overlapped the (a) window from 21:22:35Z to 21:32:57Z; the (a) verdict is pass/fail only, and no timing figures from it are claimed. The post-typing lock classification (behaviour as of the accepted tree): a cancelled or lock-bound wait before typing is Pending (exit 75, safe to resend); after typing began for the current prompt, the same given-up wait is PossiblySubmitted (exit 76, must not be resent), proven by agent::tests::a_queue_lock_failure_after_typing_is_possibly_submitted_not_pending and agent::tests::a_typed_prompt_whose_commit_lock_failed_is_quarantined_by_the_next_drain_without_retyping. Follow-up: #1434 inflight-only-queue-recovery (watch passes never start a drain when a claimed prompt is the only queue item, so start-of-drain recovery can wait behind unrelated traffic).
