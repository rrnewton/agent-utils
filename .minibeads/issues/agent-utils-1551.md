---
title: 'agentctl-chat-launch-wallclock-tests: python chat-launch tests wait on wall-clock budgets and fail under load'
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

[muse ultra] Two Python chat-launch integration tests fail under load while passing alone, against owner rule 346 (gating tests must be robust on a hot host; nothing real-time-sensitive may gate). `test_launch_sigterm_exits_within_a_few_seconds` failed in a full gate with `subprocess.TimeoutExpired` after its 120 s `communicate` bound, after its 300 s startup guards had passed. `test_launch_bridge_killed_first_is_an_error_and_stops_coordinator` did not see the bridge process within its 60 s wait under full parallel validate load, while passing solo in 19 s on the same HEAD.

# Design

Exit and observable state are the conditions; every bound only reports a hang. The promptness verdict for a stop comes from the bridge's own log lines (a clean stop logs that it stopped after the termination request; a stop the termination guard had to abandon logs that it did not stop within its grace), never from seconds measured by the test process, which a loaded host can starve. Short hang guards in the same file are raised uniformly to the file's generous 300 s. The same family's remaining elapsed-seconds verdict and sleep-based staging are converted to the same condition waits.

# Acceptance Criteria

- `test_launch_sigterm_exits_within_a_few_seconds` and `test_launch_bridge_killed_first_is_an_error_and_stops_coordinator` pass under a capped CPU burner.
- No test in the file judges behaviour by elapsed wall-clock time measured in the test process.

# Notes

Filed together with #1430 agentctl-lib-tests-load-robust, the Rust edition of the same load-robustness work, in one delivering commit.

## Resolution

[muse ultra] Fixed in `py/tests/test_agentctl_chat_launch_rust.py`. `test_launch_bridge_killed_first_is_an_error_and_stops_coordinator`: bridge discovery wait 60 s -> 300 s (also in the embedded outer-kill helper copy). `test_launch_sigterm_exits_within_a_few_seconds`: readiness is the bridge process itself (not any process naming the state, which the launcher's own argv also does), the exit bound is the file's generous 300 s hang guard, and promptness is judged by the bridge log's own clean-stop line with no termination-guard abandonment line. Same family in the same file: `test_run_sigterm_with_a_plugin_that_ignores_close_exits_within_five_seconds` lost the file's only elapsed-seconds verdict (`took < 5`, measured by the starvable test process) for the forced-shutdown log line plus process exit; `test_run_sigterm_with_a_delivery_in_flight_exits_within_five_seconds` waits for the request record's delivery-start stamp instead of sleeping 2 s (which under load could SIGTERM before delivery and pass vacuously). All remaining 60/120 s bounds in the file are uniformly the 300 s hang guard, and `_wait_for` uses a monotonic clock.

Acceptance: under a capped burner (16 reserved cores via cpuset-alloc, 32 `stress --cpu` spinners pinned to the same cores as pytest), all four tests passed in 445 s; the full file (14 tests) also passes capped unloaded. Strict mypy clean. Landed with #1430 agentctl-lib-tests-load-robust.
