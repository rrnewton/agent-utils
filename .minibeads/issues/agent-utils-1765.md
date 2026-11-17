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

- Step 1 (this issue's first landing): take the named checks out of gating with a clear skip reason naming this issue, through the selection each gate already honours, so they still run on demand. Pytest cases carry `load_quarantine` (excluded by default; run with `WRKSLOTS_INCLUDE_QUARANTINED=1` or `-m load_quarantine`). The standalone step carries `disabled-optin` (excluded from every default and `--all` run and named as skipped; run with `python3 scripts/validate.py --include-disabled`, `AGENT_UTILS_INCLUDE_DISABLED=1`, or directly with `python3 scripts/check_standalone.py --subset wrkslots`). The standalone removal waits on its completion condition (the slot is gone and status reports no active slot) behind a generous backstop; exhausting the backstop reports no result (exit 75) instead of a failure. Nothing else in step 1.
- Step 2 (after step 1 lands and is reported): find the real cause of the slice refusals (TasksMax and pids accounting of nested transient slices under a contended user manager, or a rate limit on unit creation), make the tests robust with condition-based waits only, then put them back. Report before starting step 2.

# Acceptance Criteria

- Step 1: the named pytest cases are excluded from default shard runs, named as quarantined under this issue, and run unchanged with the opt-in; the standalone wrkslots step is excluded from default and `--all` validation, named as skipped under this issue, and still runs end to end on demand; its removal verdict comes from the completion condition, with only a generous backstop that reports no result.
- Step 2 return bar (owner rule 346): each returned check passes repeatedly with the host above 75% CPU on 316 hardware threads, using condition-based waits only (deterministic; no wall-clock budget decides the verdict), with the run count and load recorded in Notes before its quarantine mark is removed.

# Notes

Related: #1267 wrkslots-load-robust (the case-level quarantine this reuses), #1250 vibe-talk-timing-robust (the `disabled-optin` mechanism), #1748 standalone-wrkslots-scenario-wall-clock-budget (the scenario-only budget exemption this builds on). Step 2 starts only after step 1 has landed and been reported.
