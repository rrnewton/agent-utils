---
title: 'agentctl-goal-wait-unbounded: agentctl goal can wait forever, ignore SIGTERM and outlive its target'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T17:30:00.000000000+00:00
updated_at: 2026-11-08T22:32:34.000000000+00:00
closed_at: 2026-11-08T22:32:34.000000000+00:00
---

# Description

A consumer reported (2026-11-08) an `agentctl goal <agent> --file F` process, started 2026-11-07 about 08:40Z, that slept in `pause()` (wchan `__ia32_sys_pause`) for 32 h 43 min, was re-parented to the user systemd, and ignored SIGTERM; it needed SIGKILL. Its target agent had since been archived (`unknown agent`). The binary was the shared `~/.local/bin/agentctl`, which at that time was the services snapshot built from commit f8b834e5 (per the host's global-agentctl history), so later fixes may not apply; check whether current main still waits this way.

# Design

- Goal waits are bounded: a default timeout and an explicit deadline flag, documented in `--help`.
- While waiting, the command notices the target agent being stopped or archived (or its record disappearing) and exits with a clear error naming what happened.
- SIGTERM (and SIGINT, SIGHUP) end the wait promptly; nothing blocks all signals across the wait.
- If the wait parks in `pause()` or a signal wait, make sure its wake-up sources include the target's state change and the deadline.

# Acceptance Criteria

- A test: the target agent is archived mid-wait and `goal` exits promptly with the clear error.
- A test: the deadline passes and `goal` exits with a timeout error.
- A test: SIGTERM during the wait ends the process within a second or two.
- `--help` documents the default timeout and the deadline flag.

# Notes

Reported by a consumer's coordinator through the lead coordinator. Related: #1061 (another long-lived process ignoring SIGTERM).

## 2026-11-08 findings (cl-start)

Mechanism of the reported process: it was not a goal wait. agentctl runs each bounded Herdr control command through the chat-subscription plugin's `ProcessPluginChild`, which clone3s an atomic supervisor. The supervisor keeps its host's command line (so it showed as `agentctl goal <agent> --file F`), blocks every blockable signal and sits in `pause()`. Until #1061 chat-launch-outer-kill-leak (d83ae91c, landed 2026-11-08), a supervisor whose host was killed mid-command (for example by an outer timeout's SIGKILL) was never taken down: it was re-parented to the user manager, ignored SIGTERM and stayed parked indefinitely. That matches every reported symptom. The reporter's binary, f8b834e5, predates d83ae91c (checked with `git merge-base --is-ancestor`). Since d83ae91c the supervisor polls `getppid()` every 0.5 s and SIGKILLs its group once its host is gone. **Consumers must refresh their agentctl past d83ae91c.**

The same mechanism explains the test watchers the coordinator killed before removing a slot on 2026-11-08: `agentctl-service-stop-*` and `w870.*` watch-agents from 11-04 and 11-07, and a `test_launch_bridge_killed_first` chat run from 11-03. All of them predate d83ae91c. The service-stop tests kill the watch-agents process group, but the supervisor leads a private group of its own, so the group kill missed it. A run of `rs/agentctl/tests/service_stop.rs` on current main leaves no process naming its scratch registry.

What current main still waited on without bound: `goal` took the agent's lifecycle lock with no limit, while `send` bounds the same lock by its send deadline (#702).

Changes:

- `goal` takes the lock through `delivery_lock` under the send deadline, as `send` does, and passes the remaining deadline to the delivery.
- `goal` gains `--deadline` (default: `send_deadline_seconds` in the registry's settings.json, else 600 s), documented in its help.

An archive or stop of the target needs the same lock. So, while waiting, `goal` either gives up at the deadline (exit 75, nothing recorded) or, once the archive releases the lock, exits at once with `unknown agent "<name>"`. It never recreates the archived record.

Tests are in `rs/agentctl/tests/goal_bounds.rs`:

- a held lock gives up at `--deadline 2`; with the fix reverted the test hangs;
- an agent archived mid-wait gives a prompt `unknown agent` error, with no record recreated;
- the help documents `--deadline` and its default;
- SIGTERM during a hung Herdr command ends `goal` within 2 s and leaves no process behind;
- SIGKILL during a hung Herdr command leaves no supervisor behind.

## Resolution

Landed as b324d32d. `goal` takes the agent's lifecycle lock under the send deadline and has `--deadline` (default `send_deadline_seconds`, else 600 s). A goal waiting behind an archive exits with `unknown agent` once the lock is released. SIGTERM and SIGKILL during a hung Herdr command leave nothing behind, given #1061's supervisor fix (d83ae91c). The reported 32 h process was that pre-d83ae91c orphaned supervisor; consumers must run an agentctl that includes d83ae91c. Tests: `rs/agentctl/tests/goal_bounds.rs`, 5 cases. Gate on 67933774: all but repository.format.rust passed. The rewrap amend ed8deb7b passed a solo rerun of format and rust.agentctl.test (2406 cases).
