---
title: 'wrkslots-load-sensitive-gate-tests: live sandbox, standalone removal and ownerless-plan checks gate on wall-clock under load'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
- test-robustness
created_at: 2026-11-17T04:00:00-05:00
updated_at: 2026-11-17T04:00:00-05:00
---

# Description

[watermelon-full-preview] Owner rule 346: no real-time or load-sensitive test gates; every gating check must pass on a hot machine (over 75% CPU on 316 hardware threads), and a load-sensitive check is disabled until it is robust, then brought back. Tonight's gates went red repeatedly on checks that pass serialized or solo, costing hours of reruns:

- Live sandbox cases in `py/wrkslots/tests/test_slot_images.py` fail at box startup with systemd refusing the box slice (exit 126, `wrkslots run: limits dagrun-wrkslots-...slice: TasksMax=256|8192`) under gate concurrency: `test_sandbox_view_confines_writes[root]`, `test_coordinator_box_confines_writes[worktrees-root|project-root]` and `test_other_slots_and_the_registry_are_read_only_from_a_box[*-root]` (all four root layouts), seen at 1:28 AM and 3:42 AM ET on different shards. The `userns` variants create their slice through the same user-manager path, so they share the mechanism and are quarantined with the root variants until step 2 proves otherwise. The root-mode live cases also run the helper from the worktree via passwordless sudo and, with the passwd-only home, can touch the owner's real home; leaving gating removes that from every gate (the home fix itself is tracked separately; this issue does not touch `sandbox.py`).
- `packages.standalone.wrkslots`: a remove-agent-batch item with a 60-second wall budget (hit 113 s under load; 73 s solo), so the standalone lifecycle verdict depended on host load instead of what it proves.
- `py/wrkslots/tests/test_recover_ownerless_write_rust.py::test_ownerless_worktree_plan_matches_python` (systemd busctl under load).
- `py/wrkslots/tests/test_census_cost.py::test_audit_record_with_precomputed_folds_never_refolds` fails on a one-second boundary (`heartbeat_age_seconds` 27679745 vs 27679744) under gate load: it compares two audit rows read from the wall clock seconds apart. Fixed in step 1 with a fixed clock in the test (no quarantine needed).
- `rs/agentctl` `tmux_lifecycle::tmux_backend_runs_the_full_agent_lifecycle` times out under gate load (480 s, gate 4:34 AM ET); its slowness and leaked tmux server are tracked separately as #1590. Taken out of gating in step 1 with `#[ignore]` naming this issue and #1590; not fixed here.

# Design

- Step 1 (this issue's first landing): take the named checks out of gating with a clear skip reason naming this issue, through the selection each gate already honours, so they still run on demand. Pytest cases carry `load_quarantine` (excluded by default; run with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`); the tmux lifecycle case carries `#[ignore]` naming this issue and #1590 (run with `cargo test -- --ignored`). The standalone wrkslots step stays in gating: its removal now waits on its completion condition (the slot is gone and status reports no active slot) behind a generous backstop, and exhausting the backstop reports no result (exit 75) instead of a failure, which removes its load sensitivity without dropping the installed-package coverage. Nothing else in step 1.
- Step 2 (after step 1 lands and is reported): find the real cause of the slice refusals (TasksMax and pids accounting of nested transient slices under a contended user manager, or a rate limit on unit creation), make the tests robust with condition-based waits only, then put them back. Report before starting step 2.

# Acceptance Criteria

- Step 1: the named pytest cases are excluded from default shard runs, named as quarantined under this issue, and run unchanged with the opt-in; the tmux lifecycle case is ignored by default naming this issue and #1590 and runs with `--ignored`; the standalone wrkslots step stays in gating and its removal verdict comes from the completion condition, with only a generous backstop that reports no result.
- Step 2 return bar (owner rule 346): each returned check passes repeatedly with the host above 75% CPU on 316 hardware threads, using condition-based waits only (deterministic; no wall-clock budget decides the verdict), with the run count and load recorded in Notes before its quarantine mark is removed.

# Notes

Related: #1267 wrkslots-load-robust (the case-level quarantine this reuses), #1250 vibe-talk-timing-robust (the `disabled-optin` mechanism), #1748 standalone-wrkslots-scenario-wall-clock-budget (the scenario-only budget exemption this builds on). Step 2 starts only after step 1 has landed and been reported.

Tracked for step 2 only (not fixed in step 1): `vibe-talk.check.thread-pin-picker-browser` flakes under load (failed 2 of 4 full validates on an unrelated tree, 11 s, exit 1); examine it as a vibe-talk browser check in step 2.

## Step 2 root cause: contended user manager refuses slice limits once

`apply_slice_limits` started each slice and set its properties exactly once; inside a gate (dozens of steps creating slices under `dagrun-*` parents on a hot host) a transient manager refusal (D-Bus timeout/NoReply, destructive transaction, connection contention) failed the box at once (exit 126). Not reproduced outside gates in 32 concurrent runs at load 25-45. Fix: `_set_slice_limits` retries only the named transient markers, waiting on the slice's `LoadState` between tries with a 180 s backstop that raises the existing refusal with the last error and the try count, logging every retry. The three live sandbox cases are un-quarantined; deterministic fake-systemctl tests cover transient-then-success, permanent-fails-at-once, and transient-until-backstop.

## Step 2 root cause: ownerless plan reads starved busctl evidence

The differential's Python oracle reads user-manager evidence through bounded busctl queries; a contended manager starves the budget and the editions diverge. Test-only fix: `_wait_for_user_manager` waits for the manager to answer behind a 120 s backstop that skips; the case is un-quarantined.

## Step 2 follow-up: chat_service idle-backstop waits on the wall clock

`rs/agentctl` chat_service idle-backstop tests (`idle_backstop_case` and its callers in `rs/agentctl/src/chat_service.rs`) assert on wall-clock time ("waited 5.52s, got 0 reads") and failed under full-host load in a local validate. Same rule-346 class: wait on a condition (or a fake clock) with a generous backstop. Taken after the current step-2 change lands; kept in this issue (no new number needed).

## Step 2 follow-up: merge-queue intake window waits on timing

`rust.merge-queue.test` `cli_intake_window::an_ejected_change_resubmitted_at_a_new_head_is_validated_in_the_same_run` failed in a loaded full validate ("queued" vs "landed" at `cli_intake_window.rs:428`) and passes solo in 3.2 s. Likely a timing assumption in the intake window; make it wait on a condition. Taken after the current step-2 change lands.

## Step 2 follow-up: census binding deadline asserts on the wall clock

`py/wrkslots/tests/test_census_binding_deadline.py::test_twelve_real_roots_stop_binding_at_the_default_five_second_deadline` asserts every audit-cache open started before a wall-clock 5 s deadline; it failed in a loaded gate and passes 5/5 solo on two trees. Same rule-346 class; taken after the current step-2 change lands.

## Step 2 follow-up: validation-exclusion fixture asserts on a census bound

`test_validation_exclusion_fixture_preserves_host_root_boundary` (a 5 s census bound) fails identically on unmodified `github/main` under load. Same rule-346 class: wait on a condition or use a fake clock. Taken after the current step-2 change lands.

## Step 2 follow-up: journal actor liveness waits near a wall budget

`py/wrkslots/tests/test_journal_actor_liveness.py::test_wait_lock_waits_for_a_live_journal_then_proceeds` failed in a loaded gate (line 178) and passed 5/5 solo on two trees at 24-43 s each, suggesting a wall-clock budget near its runtime. Same rule-346 treatment: condition waits or a fake clock.

## Step 2 root cause: browser checks sample on wall deadlines

`thread-pin-picker-browser` polled conditions on 10 s wall deadlines (11 s exit-1 failures); its waits now keep the same conditions behind 120 s backstops. `pack-still-browser` recorded frames for 900 ms of wall time, so a starved main thread produced no slide (`460.4375 -> 460.4375`); it now records until the slide condition holds (with a frame-count backstop), assertions unchanged.

## Step 3: the five follow-ups, and floating-chip-overlap

Step 3 is test-only throughout. Load proof for each: 10/10 runs with CPU burners pinned to the test's own CPU set (never beyond it), on this 316-thread host.

- chat_service idle backstop: `idle_backstop_case` showed the second reply after half a second without a new read *start*; a read descheduled between its start and its screen capture captured the new reply itself and stored it with zero reads in between. The fake client now records read start instants and a completion count (a drop guard, so every exit path counts); the reply appears only once every started read has completed, and the callers judge the interval between the loop's own previous and storing reads.
- merge-queue intake resubmission: the third validation start the test waited for is the culprit-search probe, and change 2 rebuilt onto the gate the failing change had already opened, so under load the run landed change 2 and drained before the resubmission was recorded, leaving change 1 queued. Change 2 now carries two files (its rebuilt candidate is held by its own closed gate while the resubmission is recorded, as its sibling tests already did), the fixed head is created before the run, and the resubmission wait counts the second stacked start. Reproduced before the fix: 1/3 under a 4-CPU burner failed queued-vs-landed.
- census binding deadline: the twelve-root test now patches `time.monotonic` with a fake clock advancing 0.1 s per binding start, reaching the shared 0.5 s allowance after exactly five starts however the real work is scheduled; the elapsed-time assertion is gone and the start counts are exact.
- validation-exclusion fixture: its two census budgets copied the production 5 s bound, so a loaded census refused instead of returning the census whose content the test asserts; both are now 120 s hang backstops. (The gate runs this test inside a PID namespace, where the census sees only the shard's processes.)
- journal actor liveness: the adopt test now observes `_wait_for_control_change` to prove the adopt is inside its journal wait before clearing the journal, instead of checking thread liveness after the fact; the 30 s wait-lock and joins are 300 s hang backstops on a test whose setup alone takes 24-43 s.
- floating-chip-overlap browser (added to step 3 after two gate failures on untouched main): its foot wait demanded the last card's reaction strip end above the toast's top. Under load the pill's rise over the composer lands after the scroll (resize observers delivered late), leaving the strip wholly below the pill — clear of it, which is all the walk asserts — while its bottom stays below the pill's top, so the proxy never held and the wait expired against a state that was already good. The wait now polls the asserted condition itself. Reproduced before the fix: 1/4 passed under a 4-CPU burner.
