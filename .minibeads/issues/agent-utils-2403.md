---
title: 'team-shepherd-smoke-kill-race: recovery_smoke_records_profiles_and_start_recovery_claims_once sends SIGTERM to the supervisor and asserts supervisor-dead at once, without waiting for it to exit'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- tests
- validation
created_at: 2026-11-27T15:46:00-05:00
updated_at: 2026-11-27T21:05:00-05:00
closed_at: 2026-11-27T21:05:00-05:00
---

# Description

`rs/agentctl/tests/team_shepherd.rs`, `recovery_smoke_records_profiles_and_start_recovery_claims_once`:

1. It starts the recovery supervisor and checks that a second call joins it (exit 75, `already-running`).
2. It stops the supervisor with a plain `kill <supervisor_pid>`, which sends SIGTERM and does not wait (about line 805).
3. Straight away it runs `agentctl recovery status` and asserts exit 75 with a `supervisor-dead` headline (line 825).

Status reports `supervisor-dead` only when the claim's owner is no longer alive (`owner_dead` in `rs/agentctl/src/cli.rs`, through `team_shepherd::claim_owner_alive`). Nothing makes the test wait for the SIGTERM to take effect. On a loaded host the supervisor is often still running when status reads the claim. Status then answers `verdict=no-stamp` with exit 0, and the assertion fails.

Seen on the #2340 gate on 647722c3 (2026-11-27, load1 about 206-217), in step `rust.agentctl.test`:

```
panicked at agentctl/tests/team_shepherd.rs:825:5:
assertion `left == right` failed: {... "headline":"recovery: verdict=no-stamp page=unknown", "claim":{... "takeover":"adopted"} ...}
  left: 0
 right: 75
```

3148 of the step's 3151 cases passed. #2340 does not change how this test gets its binary: it uses `env!("CARGO_BIN_EXE_agentctl")` from `cargo test`, not the adapter cache.

The test is timing-sensitive: whether it passes depends on how fast the kernel delivers SIGTERM and the supervisor exits, compared with the next command's start. The owner's rule (reply 346, 2026-11-10) forbids that in gating checks: tests must pass on a hot machine and wait on events or conditions, never on luck or a fixed sleep.

# Design

- After the `kill`, wait for the supervisor to exit before running status:
  - poll its liveness with a generous deadline (120 s, like the file's other waits), and treat a zombie as exited, as the file's `pid_alive` helper does (`/proc/<pid>/stat` state `Z`);
  - or use pidfd/waitpid where the test can.
- If it is still alive at the deadline, fail with a message that says so, rather than a status mismatch.
- Never a fixed sleep.
- Keep the cmdline check that guards the kill (the test kills only its own scratch supervisor).
- Check the rest of the file for the same pattern: any `kill` followed at once by an assertion about the process being dead. Fix those the same way.

# Acceptance Criteria

- The test waits on a condition (the supervisor's exit, with a generous deadline) between the kill and the `supervisor-dead` assertion. It contains no fixed sleep.
- It passes repeatedly with the host above 75% CPU (record the load1 and the run count in the closing notes).
- `rust.agentctl.test` passes in a gate.

# Notes

- Found by cl-shardcap while triaging the red #2340 validation-adapter-cache gate (1795786086386213333-9d85f162). coordb classed it as an owner-346 test bug and pre-assigned this number.
- Related: #2065 hot-host test inventory.

## Resolution

Landed as f267d901 on 2026-11-27. The landing replayed 6d3ba817, gated on 37078368. The change is in the test only (`rs/agentctl/tests/team_shepherd.rs`).

- **The wait.** After the guarded `kill`, the test polls `pid_alive(supervisor)` until the supervisor has exited, with a 120 s bound like the file's other waits. It fails naming the supervisor if it outlives the bound, and it has no fixed sleep.
- **The helper.** `pid_alive` moved from inside another test to file level, so both tests share it. It treats a zombie as exited, the way `claim_owner_alive` does.
- **The rest of the file.** No other test in it asserts a process is dead right after killing it.

Evidence per acceptance criterion:

- **A condition, not a sleep.** A local repro, never committed, makes the race certain: it sends SIGSTOP to the supervisor before the SIGTERM and SIGCONT from a thread 5 s later.
  - On main the test fails at the gate's assertion: headline `recovery: verdict=no-stamp page=unknown`, exit 0 against 75.
  - With the fix and the same patch it passes in 8.9 s.
- **Hot host.** The whole `team_shepherd` test file passed 16 of 16 on 3 consecutive runs at load1 309, 302 and 268 at their starts and 302, 268 and 274 at their ends. That is above the 237 that counts as 75% of 316 hardware threads.
- **Gate.** Gate `1795823762731645589-18d789d3` on 6d3ba817 passed with validate OK and no failed, incomplete or known-red steps. `rust.agentctl.test` passed 3179 of 3181 cases with 0 failed, `recovery_smoke_records_profiles_and_start_recovery_claims_once` among them.
