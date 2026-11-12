---
name: team-restore
description: Bring an agent team back after an outage (reboot, a restart of the systemd user manager, Herdr or bridge failure) with agentctl restore, verify it really came back, and avoid two restorers acting on one team. Use when a team's coordinator stops answering, its chat bridge restart-loops, its lanes are missing from Herdr, a host just rebooted, or the owner asks for a team to be restored. Harnesses extend this skill with their own hosts, teams and commands.
---

# team-restore

A **team** is a directory with one agentctl registry (`<team>/.agentctl`): the agent records, launch
profiles and `restore.json`. The team is the unit of restore. Its **lead** is its designated coordinator.
Its **units** are its systemd user services (chat bridge, watcher), started after the agents. Restore
brings back every agent whose record says it was running at shutdown, then the units.

The installed command is the authority for flags and exit codes: `agentctl restore --help`.
This skill is the protocol around it.

## 0. One restorer per team

Before you touch anything, find out whether someone is already restoring the team:

- Ask the team's lead (agent-teams for another host's team, `agentctl send` for your own). If the lead
  answers, it owns recovery: give it what you've found and stop.
- Check the team's boot restore unit (`systemctl --user status <restore unit>`), any recent restore report,
  and any freshly moved state directories (for example `bridge.old-*`). Signs of another actor mean you stop
  and talk to it.
- When you help from another host, say what you're about to do *before* you do it, and stop at once if you
  find someone else's change under way.

## 1. Diagnose (read-only)

- Is the user manager up, and did it restart? Compare `systemctl --user show -p ActiveEnterTimestamp`
  of `default.target` with the last restore report's `boot_time`.
- Agents: `herdr agent list` (which panes have a live harness) against
  `agentctl --registry R list` (what should be running).
- Units: `systemctl --user list-units 'agentctl-*' --all`. Note any unit in `activating (auto-restart)`.
- The boot restore unit's result and exit code (0 done, 74 done with refused lanes, 75 incomplete). A run
  that exited quickly having revived nothing usually replayed an older report.
- Bridges: `agentctl chat health --bridge-state <dir>` and the tail of the bridge log.

## 2. Dry run

```sh
agentctl --registry R --herdr-bin H restore --dry-run
```

Read the report before running anything:

- `desired_state_source` must be **registry-derived**. If it says `pinned-list`, `restore.json` has an
  `agents` key that overrides the records and will park everything else. Back the file up, remove the key,
  and dry-run again. Lanes that should deliberately stay down go in `exclude`.
- If `action` is `already-restored` / `replayed: true` but agents are visibly down, the guard thinks this
  boot was already restored. That's what happens after a user-manager restart, which keeps the boot id.
  Add `--force`. It only bypasses the replay; it never overrides a refusal.
- Read `refused` and each agent's `reason` now, not after the real run.

## 3. Run it

```sh
agentctl --registry R --herdr-bin H restore [--force]
```

Run it under nohup or with a generous timeout (it can take minutes), and keep its JSON report.

## 4. Verify: a restore isn't done until all of these hold

1. Every desired agent is live in Herdr, with the right profile, effort and launch environment. Look at the
   status bar of anything that should run at a raised effort level.
2. Every unit is active, and every chat bridge reports `chat health` OK **with its target agent present**.
   A bridge whose target was missing during the outage has often stopped on a stream gap since (below).
3. Messages queued to agents before the outage were delivered, not archived. Check the revived lead's
   inbox archive.
4. The watcher is running and reports no unexplained dead agents.

## 5. Known failures and their fixes

- **Bridge restart-loops on `agent_not_found`:** its target agent isn't in Herdr. Restore that agent first;
  the bridge recovers on its next restart.
- **Bridge stops on "unrecoverable stream gap" / retention overflow:** it was down longer than the event
  stream keeps events. Stop the unit, move the state directory aside (`bridge.old-gap-<date>`), and
  re-initialise with `agentctl chat init --config OLD/bridge.json --from OLD`. `--from` carries the event
  sinks over. Then check the channel by hand for messages the gap swallowed.
- **Revive refuses "death unverifiable" or "pane/cwd changed":** `agentctl stop NAME`, then
  `agentctl start NAME --profile P --resume SESSION`.
- **A lane came back at a lower effort level:** its profile's launch environment wasn't replayed. Restart
  it with stop + `start --profile P --resume SESSION` at a safe point (no gate running).
- **`agentctl list` fails or the watcher reports an odd agent:** a non-agent directory under `agents/` (for
  example an old restore report directory). Move it out; don't delete reports.

## 6. Tell people

- Tell the team lead what you did, what's still refused, and anything you couldn't verify.
- The lead tells its lanes about the outage: re-check gates that were running, rebase, and resume only the
  interrupted step. Pre-outage gate tickets are left to their reaper, never deleted by hand.
- Tell the owner in one message: cause, duration, what came back, what's left.

## Killing a stuck process: `kill-pid`, never a raw `kill`

When a process must be stopped by PID, use `agentctl kill-pid`, never a raw `kill`, `pkill` or
`killall`. Record the process's command line (`/proc/PID/cmdline`) and start time (field 22 of
`/proc/PID/stat`) first, then:

```sh
agentctl --registry R kill-pid PID --expect-cmdline 'TEXT' --expect-start TICKS
```

It refuses — printing what it refused and why — unless the command line and start time still match
(so a reused PID is refused), and it always refuses the user manager (`systemd --user`), any
`user@UID.service` main process, the Herdr server, the main PID of a unit this team doesn't own,
and any process whose working directory sits in another team's registry or slots or outside this
team's root. Every kill it performs is logged to `REGISTRY/kill-pid.jsonl`. A refusal is the
answer: identify the process properly or hand it to the team that owns it; never work around the
guard with a raw signal.

## Never

- Never signal a process except through `agentctl kill-pid` with its recorded command line and
  start time. Above all, never the user manager (`systemd --user`): killing it stops every team
  on the host.
- Never restore another team's agents while its lead is alive. Hand it what you found instead.
- Never rewrite `restore.json` from the live list (`restore --write-agents`) as routine. A pinned list goes
  stale and silently parks lanes at the next outage.
- Never delete restore reports, bridge state or archived inboxes while recovering. Move them aside.
