---
title: 'headless-identity-exec-race: a headless restart pins the launcher process, not the harness'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T16:20:57+00:00
updated_at: 2026-11-10T16:20:57+00:00
---

# Description

[opus 5.5] A headless agent's record can name a process that is not its harness after the wrapper starts one, and then every check that finds the agent through that record fails.

Seen on 2026-11-10 during a services turnover. The headless wrapper unit was restarted on a new build and started its harness (the same session, resumed). The record's `harness_identity` still named the previous run's harness process, which was gone. The wrapper's event log held one `record_harness_identity` error: "observe harness process N: custom process N identity changed while it was inspected". Effects until an operator re-pinned the record by hand:

- `agentctl coordinator show` reported the dead process.
- `agent-teams check` reported "delivery target ... is not live (its pinned harness process is gone)".
- Every `agent-teams send` and `reply` from the agent was refused as sender-not-coordinator, because sender proof walks the caller's ancestry to the pinned identity.
- Inbound queue delivery kept working, so the agent looked healthy from outside.

Cause: the wrapper spawns `--claude-bin`, and the default on this host is a short shell launcher that execs the real harness binary. `RecordIdentity::harness_spawned` (rs/agentctl/src/subagents/headless.rs) observes the new PID immediately after spawn:

- If the observation lands before the exec, it pins the shell's executable identity. Liveness checks that compare the executable then fail as soon as the exec happens.
- If it lands during the exec, the observation errors as "identity changed while it was inspected".

Either way the hook runs once and is never retried, and the wrapper's `harness.json` holds the same pre-exec identity. The first run of the same agent that day pinned the launcher's identity (a shell executable inode); the restart hit the race.

# Design

- After spawn, re-observe the harness until its identity is stable: either retry with a short bounded backoff until two observations agree and the executable is no longer an interpreter, or pin on the harness's first stream-json `system`/init event, by which time the exec is done.
- Record the post-exec identity in both the record and `harness.json`. If pinning still fails after the bounded retry, keep retrying in the background rather than leaving the previous run's identity in place; a stale identity from an earlier run must never survive a new spawn. At minimum, clear it.
- Resolve `--claude-bin` to the real binary in `install-units --headless` and in `run-headless` (follow symlinks; when the target is a launcher script, note it in the unit comment or refuse with a remedy), so the spawned process is the harness itself.
- Tests: a fake launcher that sleeps briefly and then execs a fake harness. The record must end with the harness's identity after a first start, after a restart (the stale identity of the previous run must be replaced), and after an unexpected harness exit and restart. `agent-teams` liveness and sender proof must accept the new identity.

# Acceptance Criteria

- Restarting a headless wrapper whose `--claude-bin` is an exec'ing launcher leaves the record naming the live post-exec harness, with no operator action.
- No path leaves a previous run's `harness_identity` in the record after a new harness spawns.
- `install-units --headless` writes the resolved harness binary.

# Notes

Coordinate with #1216 agentctl-effort-option, which changes the run-headless harness argv for --effort; whichever lands second rebases.
