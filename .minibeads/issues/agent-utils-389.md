---
title: 'gate-flakes: landing-gate tests that fail under host load and pass alone'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-20T07:43:08.987259053+00:00
updated_at: 2026-10-20T12:06:52.199282351+00:00
closed_at: 2026-10-20T12:06:52.199282060+00:00
---

# Description

[muse watermelon] On 2026-10-19 scripts/validate.py gates on a loaded host (load 30-130, several gates at once) failed again and again on tests unrelated to the change being landed, each passing alone; one lane needed 16 gate runs. The ci-repair lane (#359-#366) fixed the message-cache flakes and CI masking. These remain. Make each deterministic under load without weakening what it checks: wait on the condition asserted, not a fixed sleep or deadline. Do not raise timeouts as the fix unless the timeout is the thing under test.

Checklist (survey of /home/newton/agent_scratch/*-work/ gate logs from 2026-10-19):
- [ ] agentctl chat_service::tests::the_acknowledgement_worker (moved to #394 ack-delivered-empty: not reproduced in 529 loaded runs; assert now prints the whole CycleReport)_adds_the_receipt_reaction_a_printed_prompt_earned panicked at agentctl/src/chat_service.rs:6349 (left [] vs expected hash) in census-audit-work/coord-gate.log.
- [x] vibe-talk tests/message_cache_access.rs a_revoked_credential_erases_at_once_and_a_refusal_lasting_a_day_erases panicked at :1314 (Suspended vs Verified) in census-audit-work/coord-gate2.log and search-load-older-work/gate-flake1.log. Check whether #364 message-cache-test-flakes (settle_until) already covers it; if so, close with evidence.
- [x] gh-paced tests/cli.rs a_reader_stopped_after_gh_exits_still_scans_what_gh_wrote panicked at :2089 (no pause in the banner; test.lock still held after 3s) in census-audit validate-final, ci-repair gate12, pidfd-ownership validate-land2/3, val-timing-mem validate6/8 (6 gate logs).
- [x] vibe-talk approval-card-browser: page.locator(#post-confirm).wait_for(hidden, timeout=5000) timed out under load in ci-repair gate6 (approval_card_browser.py thread_list_case).
- [x] vibe-talk thread-card-header-browser: phone walk read the wrong card text at the top in done-races validate-land15 (and land9); likely asserted before the card finished rendering.
- [x] vibe-talk screenshot harness exit 31 (EXIT_UNREACHABLE) in compact-rows gate2/3/4, thread-line gate1, voice-reconnect land-gate-1.
- [x] wrkslots lifecycle host shard: test_remove_agent_batch_refuses_before_fencing_when_the_image_check_outlasts_the_budget got the 30-second /proc-scan bound reason instead of the budget reason in ci-repair gate11; test_remove_agent_batch_removes_disk_image_slots returned 1 in gate14.
- [x] agentctl test_command_timeout_terminates_descendants_holding_output_pipes read an empty child.pid (0.5s command timeout fired before the child wrote its pid) in val-timing-mem validate10.
- [x] wrkslots test_real_host_completed_removal_waits_for_the_run_process got a PID-namespace-init teardown error instead of the validation-run authority error in val-timing-mem validate9.
- [x] dagrun test_native_unreaped_zombie_and_reaping_keep_cpu_and_close_fds failed authenticating within its 1-second deadlines in pidfd-ownership validate-land.
- [x] Classify with evidence, do not fix as flakes: python-types mypy error in ci-repair gate9 (real error, fixed by the image-helper typing commit at HEAD); in-lane real regressions (provider-channels gates, pairing-link futures panic) and dagrun CPU-budget timeouts (ws-census) as surveyed.

Acceptance: each flake fails under load before the fix and passes N runs after (numbers in the commits/handoff); no assertion loosened; gate green.


Resolution notes (2026-10-20, mu-flakes; numbers in the commit messages):
- message_cache_access: already covered by #364 (commit 67e30bb2 replaced the panicking assert at old :1314 with settle_until; census coord-gate2 ran pre-fix code). Current code: 120/120 single-test and 20/20 whole-binary runs held to four CPUs. No change.
- screenshot harness exit 31: not a load flake in the surveyed logs. compact-rows gate2-4 failed '42-reply-count-and-marker' on that lane's in-progress row layout (its gates 5-7 then passed); thread-line gate1 failed '63-settings-prose' on a headings list its base predated (gate2 passed). No harness change.
- python-types (ci-repair gate9): real mypy error in that lane's tree; fixed by the image-helper typing commits at the base. provider-channels gate1/gate8, pairing-link gate2, val-timing differential (validate11), ws-census gate1: in-lane real failures at in-progress bases, green on current code where re-run. ws-census gate4 package CPU-TIMEOUT: fixed the same day by #361 package-step-cpu-budget (427f27d4).
- The three real-host validation-run tests also failed in ws-census gate2/3 and vt-ci-repair gate-1/gate-kg-1 (five logs total), same pidns-teardown shape; covered by the retry fix here.
- Not reproduced despite starved stress, no change: test_agent_remove_derives_submodule_salvage_facts_before_the_lock (ws-orphan validate2; suspected git index-stat race in the salvage fingerprint; 8/8 pass on one CPU). Any recurrence belongs in a new issue with its log.
- agentctl acknowledgement: see the #394 follow-up above.
