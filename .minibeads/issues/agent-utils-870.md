---
title: 'agentctl-service-slow-stop: watch-agents and chat run exit within seconds of SIGTERM'
status: closed
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T05:23:02.772443148+00:00
updated_at: 2026-11-05T08:00:00.000000000+00:00
---

# Description

Stopping `agentctl watch-agents` and, intermittently, `agentctl chat run` waits for systemd's 90 s stop timeout and a SIGKILL instead of exiting on SIGTERM. Every service refresh therefore risks about 1.5 min of bridge or watcher downtime.

1611c31b (the bounded admission flock) did not fix it.

**Evidence**
- The devbig030 watcher refresh on 2026-11-05 at 05:17:54Z reported "the new process started 90 s after the stop began".
- On the lead host, every watcher stop on every build from d6f0a4cf to 4a6cc263 took 91 s.
- On the lead host, a consumer team's `chat run` bridge on 59a36153 logged "plugin required forced shutdown after Close ... cleanup is uncertain" 2 s after SIGTERM (05:15:27Z), then nothing until the 90 s timeout. The same build stopped in 2 s for another bridge in the same hour, so for chat run it depends on what is in flight.

**Watcher mechanism, found by reading the code**
- `agent::install_termination_handler()` (rs/agentctl/src/agent.rs) replaces the default SIGTERM/SIGINT action with a flag, so that waits in send and drain end cleanly (#734 chat-status-disagreement).
- The handler is installed lazily, on the first in-process `send` or `drain` (subagents.rs, both call sites).
- watch-agents delivers its notices through that path. After its first notice, SIGTERM no longer terminates the process.
- The watch loop never reads the flag. Between passes it sleeps in `SystemClock::sleep` → `std::thread::sleep` (agent_watch.rs), which resumes after EINTR.
- The live watcher at rest has one thread, in `hrtimer_nanosleep`.
- So the watcher ignores SIGTERM until systemd kills it.

**chat run**
- `chat run` registers its own SIGTERM flag (chat_launch.rs) and also does in-process drains.
- The intermittent hang is probably the same pattern: a wait, sleep or join that never checks either flag, reached when a delivery, pane read or drain is in flight at SIGTERM.
- The "plugin required forced shutdown" line says the plugin helper's Close did not finish; check what is still joined after that.

# Design

- Give the process one termination predicate that every long wait can read: `agent::termination_requested()`, backed by the same flag. The watch loop checks it before and after each pass, and its inter-pass sleep becomes interruptible (sleep in short slices checking the flag, or wait on a condvar or self-pipe set from the signal handler).
- Alternatively, have watch-agents install the handler at start and own its shutdown explicitly. Either way, SIGTERM during the inter-pass sleep exits at once; SIGTERM mid-pass finishes or abandons the pass within a few seconds and releases its locks.
- For chat run: on SIGTERM, list every thread and child that main joins or waits on (delivery workers, the reply worker from #858, the plugin helper child, Herdr socket reads, drain lock waits) and bound each one, so the process exits within 5 s even when the plugin's Close is uncertain. Log what was abandoned.
- Keep #734's guarantee: a send or drain cut short by termination leaves its prompt pending and releases its locks.

# Acceptance Criteria

- A test starts `watch-agents` (not `--once`), lets it send at least one notice (so the handler is installed), sends SIGTERM during the inter-pass sleep, and asserts exit within 5 s. A second variant sends SIGTERM mid-pass.
- A test starts `chat run` with a delivery or drain in flight, sends SIGTERM, and asserts exit within 5 s, with the pending request still pending afterwards.
- A deploy-side check: `agentctl services refresh` of the watcher and of a bridge reports the new process starting within a few seconds of the stop, on both hosts.

# Notes

Reported by the lead on 2026-11-05 at 00:18 AM ET (P1). Assigned to cl-start after #869 agentctl-test-fsync-churn.

## Resolution

[opus 5.5] cl-start, 2026-11-05.

- **watch-agents:** installs the termination handler at start, not at its first in-process send. Its clock sleeps in slices that read `agent::termination_requested()`. It saves its state and exits 0 at the next pass boundary or sleep.
- **chat run:** installs the handler at start too.
- **Guard:** both run a termination guard (`agent::spawn_termination_guard`). A stop still running 3 s (watch-agents) or 4 s (chat run) after the signal is abandoned. The guard logs what was in flight, including the owner-loop step for chat run, and ends the process as SIGTERM's default action does, so systemd counts the stop as clean.
- **Root cause for chat run, reproduced:** a provider plugin that never answers Close held the stop for its whole Close deadline: 10 s with the fake plugin, up to 120 s for a real manifest.
- **Tests:**
  - `tests/service_stop.rs`: watch-agents stops from its sleep (exit 0, state saved, about 0.04 s) and from a pass stuck in a Herdr call (about 3 s).
  - `py/tests/test_agentctl_chat_launch_rust.py`: chat run stops with a delivery in flight, and the request is never recorded as delivered. It also stops with a plugin that ignores Close; that test took 10 s before the fix.
- **Unverified:** the deploy-side check, `agentctl services refresh` of a watcher and a bridge on both hosts, waits for the next refresh after this lands.

## Follow-up (2026-11-05)

On devbig030 the refresh restarted the watcher in 0.26 s and the bridge in 0.1 s, but one bridge restart took 12.3 s. The bridge log puts that stop 56 s after startup had finished, and no guard line was printed. So it was not a SIGTERM during startup, and the log alone cannot say where the time went.
- **Found while testing:** a stop while the provider plugin had not yet said hello waited for the plugin Hello timeout (10 s), cut to 4 s by the guard. `connect_unless` in chat-subscription-plugin now abandons the Hello at once when the bridge is stopping. Tests: `a_hello_wait_ends_when_the_host_abandons_it_and_the_plugin_is_reaped` (conformance), and `test_run_sigterm_during_startup_exits_within_five_seconds[at-exec, plugin-handshake]`. Measured stop times: 0.1 s at exec; during the handshake, 4.11 s before the fix and 0.10 s after.
- **Logging:** both services now log `termination requested; stopping` and `stopped N.Ns after the termination request`, so the next slow restart shows whether the stop or the start was slow.
