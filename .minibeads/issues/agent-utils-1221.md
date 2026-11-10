---
title: 'headless-identity-exec-race: a headless restart pins the launcher process, not the harness'
status: closed
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
- Spawn `--claude-bin` as given in `run-headless` and write that launcher path in `install-units --headless`: canonicalising a symlink to the launcher script is fine, but never resolve past the launcher to the real binary, because the launcher's environment setup (on this host its `MCP_TIMEOUT` export) must run. The resolved real binary belongs in the unit comment only. (This supersedes the original "resolve to the real binary" line: resolving skipped the launcher's exports, while the post-exec re-observation already pins the right process.)
- `agentctl tail NAME --follow` must survive a record replacement, not only wrapper restarts: when the agent directory or record token changes under the same name (new token, new generation, or events/index replaced), tail prints a one-line `restarted (record replaced)` marker, resets its cursor, and follows the new log from its start. Detect replacement by the record token/instance or generation, not segment numbers, which a fresh log can sit entirely behind.
- Tests: a fake launcher that sleeps briefly and then execs a fake harness. The record must end with the harness's identity after a first start, after a restart (the stale identity of the previous run must be replaced), and after an unexpected harness exit and restart. `agent-teams` liveness and sender proof must accept the new identity. A tail test: follow, replace the record, then append to the new log, and the new events must appear.

# Acceptance Criteria

- Restarting a headless wrapper whose `--claude-bin` is an exec'ing launcher leaves the record naming the live post-exec harness, with no operator action.
- No path leaves a previous run's `harness_identity` in the record after a new harness spawns.
- `install-units --headless` writes the launcher path as given, with the resolved real binary named in a comment only, and the spawned harness inherits the launcher's exports (for example `MCP_TIMEOUT`) while its record still names the post-exec process.
- `agentctl tail NAME --follow` following an agent whose record is replaced under the same name prints a `restarted (record replaced)` marker and then the new log's events.

# Notes

Coordinate with #1216 agentctl-effort-option, which changes the run-headless harness argv for --effort; whichever lands second rebases.

## Resolution

[watermelon-full-preview] Delivered: `observe_stable` in agent-queue re-observes a spawned harness with a bounded, paced retry until two observations agree and the executable is no longer an interpreter, with a stable-interpreter fallback pinned provisionally. `RecordIdentity::harness_spawned` clears the previous run's `harness_identity` before observing, and the wrapper clears `harness.json` the same way, re-pins on the first stream-json init event, and keeps retrying an unfinished or provisional pin in its main loop, so no stale identity survives a new spawn. `tail --follow` watches the record token and agent/events directory identity plus generation regression, prints `restarted (record replaced): generation N -> M`, resets its cursor, and follows the new log from its start. Tests cover a fake exec'ing launcher on first start, wrapper restart, and unexpected-exit restart, the identity/agent-teams liveness and sender proof, binary resolution, the resolved unit, and tail record replacement.

Follow-up (same day, lead-approved): spawning the resolved real binary directly skipped the host launcher's environment setup (its only export is `MCP_TIMEOUT=60000`). `run-headless` now spawns `--claude-bin` as given (`canonical_launcher_path`: PATH lookup and symlink canonicalisation to the launcher script, never past it), and `install-units --headless` writes the launcher path with the resolved real binary (`real_binary_of_launcher`, from `resolve_harness_binary`) in a comment only. Tests prove a launcher's `MCP_TIMEOUT` export reaches the harness while the pinned identity is still the post-exec process, at both the wrapper and `run-headless` levels. `observe_stable` also requires the same non-interpreter identity continuously for about 200 ms (inside its bounded 2 s retry), and the wrapper keeps watching the pinned identity for the harness's whole life, so a launcher chain that settles slowly on a hot machine is re-pinned when it lands: the record is cleared or provisionally pinned and then corrected, never finally wrong. A harness whose final image is itself an interpreter (node, python) pins that stable interpreter identity at the retry deadline, which liveness accepts for that process generation.
