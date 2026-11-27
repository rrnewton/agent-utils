---
title: 'console-pty-launcher-race: the console_pty test launcher execs the console before its intermediate fork has exited, so under load the console finds the harness that started the gate and refuses (4 of 24 at 12 in parallel)'
status: open
priority: 2
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- console
- tests
- gate
created_at: 2026-11-27T08:52:21-05:00
updated_at: 2026-11-27T08:52:21-05:00
---

# Description

On 2026-11-27 a full gate failed `rust.agentctl.test` on one case only, while load1 was 125 to 138. The gate ran on #2338's head d8b73e16, which does not touch the console.

- The case was `console_pty::console_delivers_typed_lines_as_plain_turns`, at `tests/console_pty.rs:310`, with "timed out waiting for banner on screen".
- The console screen held only: `agentctl: console refuses to run inside a harness (claude pid N); a lane cannot type as the owner`.

The guard works as designed. `console::authenticate` refuses when `sender_header::harness_ancestor` finds `claude`, `codex` or `muse` among the console's ancestors (#670). The flaw is in the test.

- The test's `LAUNCHER` (added in 73e9f1ab) double-forks so that the console's parentage reaches init. The first process forks and exits at once. The child calls `setsid`, takes the terminal, writes its pid file and execs `agentctl console worker`.
- Nothing makes the child wait until the intermediate process has exited. Until it has, the console's parent is still the launcher. The launcher's ancestors are the test binary, cargo, dagrun, the gate and the harness that started the gate.
- If the console's ancestor scan runs in that window, it finds the harness and refuses. The test then waits out its 60 s banner timeout.

Who is exposed:

- Only a suite that runs inside a harness can fail this way. That includes every gate a lane starts.
- Load and concurrency decide how often the window is hit.
- `console_shows_a_follower_error_instead_of_silence` uses the same launcher, so it is exposed too.

Evidence (2026-11-27, from a lane shell):

- The exact gated test binary passed 20 of 20 runs one at a time.
- Run as 12 copies at once, in two rounds, it failed 4 of 24 runs. Every failure printed the same refusal, naming the same harness pid.
- In the earlier gate step logs on the host, this case passed 6 of 6 times.
- The harness is not a child subreaper. From the same shell, a double-forked orphan reparents to the user service manager.

# Design

Fix the test launcher only. The console's owner proof stays as it is.

1. Before the fork, the launcher records its own pid.
2. The child polls `os.getppid()` until it no longer equals that pid, with a short sleep between polls, before it execs the console. The parent change means the intermediate process has exited and the child has been reparented away from the suite.
3. The poll has a generous deadline (for example 30 s). Past the deadline, the child exits with a message that names the cause, so a stuck launcher fails by name instead of a banner timeout.

This puts no wall-clock budget on the outcome, only an event wait with a generous cap, so it holds on a hot host.

# Acceptance Criteria

- The launcher's child execs `agentctl console` only after its parent has changed away from the launcher.
- The repro passes with no refusal: the console_pty cases run as 12 copies at once, at least 48 runs, on a host as loaded as the gate was.
- `console.rs` (`authenticate`) and `sender_header.rs` (`harness_ancestor`) are unchanged.
- `rust.agentctl.test` is green in a gate started from a lane.

# Notes

- Found by cl-bridgeretire while triaging the #2338 gate (2026-11-27); coordb assigned the number.
- The fix is a separate small commit off main, after #2366 and #2338 land.
- Related: #670 (the console's owner-at-a-terminal proof), #2338.
