---
title: 'agentctl-chat-stop-during-reconnect-backoff: a SIGTERM that reached chat run right after exec left its owner loop retrying subscriptions until the 120 s termination guard ended it'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- chat
- validation
created_at: 2026-11-27T08:10:00-05:00
updated_at: 2026-11-27T08:10:00-05:00
---

# Description

`python.agentctl.shard-0` failed in the #2318 gate on 1b0c8419 (ticket `1795777600642083805-50cbd6b7`, 2026-11-27). One test of 1458 failed: `tests/test_agentctl_chat_launch_rust.py::test_run_sigterm_during_startup_stops_cleanly[at-exec]`. Nothing in #2318 touches agentctl or its tests.

The test starts `chat run` with `AGENTCTL_CHAT_STOP_GRACE_S=120`, sends SIGTERM as soon as the bridge process exists, and asserts that the bridge stopped by itself, so its log has no `did not stop within` line. The bridge log ended like this:

```
2026-11-27T12:34:14Z agentctl: chat provider: subscription backend failed: fake_configuration_mismatch: the conformance backend did not receive its exact host configuration; reconnecting in 32s
2026-11-27T12:34:17Z agentctl: chat output subscription: output subscriptions require a running, compatible Herdr server; retrying in 32s
2026-11-27T12:34:46Z agentctl: chat provider: subscription backend failed: fake_configuration_mismatch: ...; reconnecting in 60s
2026-11-27T12:34:49Z agentctl: chat output subscription: output subscriptions require a running, compatible Herdr server; retrying in 60s
agentctl: chat run did not stop within 120s of the termination request; abandoning what is still in flight (the owner loop was in its step: wait for provider notices) and exiting
```

The kept tail starts at 12:33:45Z, with backoffs of 2, 4, 8, 16, 32 and 60 s. So for about two minutes after the termination request, the provider and output subscriptions kept retrying, and the owner loop kept running its passes. Only the termination guard ended the process.

The owner loop's wait for provider notices lasts at most 1 s per pass. The 60 s backoff alone therefore can't have held the stop. The loop never saw a stop.

Host load (load1, from the host's one-minute sampler) was 178 at 7:33 AM ET, 186 at 7:34, 216 at 7:35 and 199 at 7:36 (316 hardware threads). That's below the 237 that counts as a hot host.

**Likely mechanism (from a code read; not yet reproduced):** `chat run` has two SIGTERM paths that start at different times.

- `cli.rs`, at `ChatCommand::Run`, calls `install_termination_handler()` and `spawn_termination_guard("chat run", ...)` before `run_with_settings`. From then on, a SIGTERM sets the process-wide termination flag and arms the guard.
- The owner loop stops on `StopState::stop()`. Only the chat signal worker calls that, and it learns of the signal through `signal_hook::iterator::Signals`. That worker is created later, inside `run_with_settings` (`spawn_signal_worker`, `Signals::new([SIGINT, SIGTERM])`), after state loading and trace setup.
- A signal delivered before `Signals::new` registers isn't reported by its iterator. A SIGTERM in that window therefore sets the flag and arms the guard but never reaches `StopState::stop()`.
- The owner loop reads only `stop.is_stopped()`. `termination_requested()` is read only in the plugin's Hello wait, `connect_unless`, which is why the `plugin-handshake` case stops cleanly.

The test sends SIGTERM as early as it can see the process. A loaded host widens the window between the two registrations, so the race shows up more often under load. A service stopped at the same moment, for example by a systemd restart just after start, would wait out its whole grace in the same way.

# Design

- **One stop for both paths.** A termination request that arrives before the chat signal worker exists has to stop the owner loop too. Two ways:
  - check `termination_requested()` right after `Signals::new` and stop at once if it is set;
  - or have the owner loop, and every wait that already reads `stop.is_stopped()`, read `termination_requested()` as well, as `connect_unless` does.
- **A deterministic regression test.** Use a test-only pause point between `install_termination_handler()` and `spawn_signal_worker`, set by an environment variable and announced on stderr or by a marker file. The test sends SIGTERM there and asserts that the bridge stops by its own route: its `stopped N s after the termination request` line, and no `did not stop within` line.
  - The test waits on the pause marker, not on the clock. That follows owner rule 346 and #1750.
  - The existing `at-exec` case stays as a smoke test.
- If the read above turns out wrong, record what the reproduction shows here before changing the test.

# Acceptance Criteria

- A SIGTERM delivered at any point after `install_termination_handler()` stops `chat run` through its own stop, never through the termination guard. The new pause-point test proves it with a generous bound.
- The test fails against the current code, and its verdict doesn't depend on host speed: it passes with the host's load1 above 237.
- `test_run_sigterm_during_startup_stops_cleanly[at-exec]` and `[plugin-handshake]` still pass.

# Notes

- Related:
  - #870 agentctl-service-slow-stop added the termination flag, the guard and these tests.
  - #1750 agentctl-tests-load-sensitive moved them to condition waits with a generous grace.
  - #291 stop-lost-wakeup is the earlier lost-stop race inside `StopState`.
- Owner rule 346: no gating check depends on wall-clock time, and every gating check passes on a hot host.
- Found by cl-shardcap in the #2318 gate. Step log: `python.agentctl.shard-0.log` in gate log directory `1795777600642083805-50cbd6b7`.
- The exclusive rerun of both red steps on the same tree passed (ticket `1795784636031325182-279d838b`, 8:10 to 8:21 AM ET): this test passed, 1458 of 1458 in `python.agentctl.shard-0`. load1 during the rerun was 107 to 154, lower than during the failure, so the pass shows only that the race is intermittent, not that the test is robust under load.
