---
name: team-restore
description: Bring an agent team back after an outage (reboot, a restart of the systemd user manager, Herdr or bridge failure) with agentctl restore, verify it really came back, and avoid two restorers acting on one team. Use when a team's coordinator stops answering, its chat bridge restart-loops, its lanes are missing from Herdr, a host just rebooted, or the owner asks for a team to be restored. Harnesses extend this skill with their own hosts, teams and commands.
---

# team-restore

A **team** is a directory with one agentctl registry (`<team>/.agentctl`): the agent records, launch
profiles and `restore.json`. The team is the unit of restore. Its **lead** is its designated coordinator.
Its **units** are its systemd user services (chat bridge, watcher), started after the agents. Restore
brings back every agent whose record says it was running at shutdown, then the units.

The installed command is the authority for flags and exit codes: `agentctl restore --help`,
`agentctl team --help`. This skill is the protocol around it. The same protocol, written for a
person on an outside team, is the recovery guide
[`common/docs/agentctl/recovery.md`](../../common/docs/agentctl/recovery.md).

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
- The whole picture in one read-only document: `agentctl --registry R team manifest`
  (schema `agentctl-team-manifest/v1`): the desired set, per-agent liveness, profile and effort,
  `record_gaps` (records missing a profile, bypass flag, declared effort or recorded session, with
  report-only session candidates), queued prompts, unit state, `lost[]` and an embedded verify. It
  never contains record tokens or launch arguments, so it is the thing to share or hand to a judge.

## 2. Dry run

```sh
agentctl --registry R --herdr-bin H restore --dry-run
```

Read the report before running anything:

- `desired_state_source` must be **registry-derived**. If it says `pinned-list`, `restore.json` has an
  `agents` key that overrides the records and will park everything else. Back the file up, remove the key,
  and dry-run again. Lanes that should deliberately stay down go in `exclude`.
- If `action` is `already-restored` / `replayed: true` but agents are visibly down, the guard thinks this
  epoch was already restored. Since #1405 the epoch is the boot plus the user manager's identity, so a
  user-manager restart (which keeps the boot id) is a new epoch and restores for real; an older build,
  or a host where the manager's identity can't be read, replays instead. Add `--force`. It only bypasses
  the replay; it never overrides a refusal.
- A report with `complete: false` and `in_progress: true` is a killed attempt's partial report, not a
  completed restore: it names the lanes that attempt already revived under `agents`. A rerun in the same
  boot resumes from it — those lanes are carried forward (`resumed: true`, listed in `resumed_lanes`)
  after being re-proved alive, and only the remaining lanes are revived. Never delete a partial report
  to "start clean"; it is the resume state (and the only boot evidence if the journal is unreadable).
- Read `refused` and each agent's `reason` now, not after the real run.

## 3. Run it

```sh
agentctl --registry R --herdr-bin H restore [--force]
```

Run it under nohup or with a generous timeout (it can take minutes), and keep its JSON report.
The boot unit's `TimeoutStartSec=1800` is a per-attempt watchdog, not a whole-team budget: restore
writes its partial report as it goes, so a timed-out and restarted unit resumes where it left off
instead of starting over. Do not raise or remove that timeout to fit a big team; if a single attempt
makes no progress (the partial report stops growing), that is a hung lane to diagnose, not a budget
to widen.

On a host and build with the deterministic driver, prefer
`agentctl --registry R team kick` for the whole recovery: it detects,
restores with the replay guard bypassed for that detection, verifies,
hands the remaining checklist to the lead on a deadline, and escalates
through the configured commands — one driver per team under its state
lock, so it is also the one-restorer guard above. It prints its
persisted state (schema `agentctl-team-driver/v1`, under
`<state root>/team-drivers/<key>/`); exit 75 means recovery is pending
with a wake armed, not failed. Kick only from the host PID namespace:
inside a team namespace it refuses rather than half-restoring. The
manual steps below remain the fallback and the meaning of its checks.
Never run a manual `restore` while the driver works: that is two restorers.

### Fixing what is left: `team act`

`agentctl --registry R team act --incident I --request-id ID <action>` queues one allowlisted
action for the driver: `facts`, `revive NAME`, `start-unit UNIT`, `reset-unit UNIT`,
`drain NAME`, `kill-pid PID --expect-cmdline T --expect-start TICKS --boot-id B`, `declare-done`,
`escalate TEXT`. The driver validates it against a fresh manifest, runs it, persists a receipt and
verifies again; it never rewrites a record or binds a session. The incident (12 hex) is
`.state.incident` in the `team kick` report.

- `--request-id` is the idempotence key: the same id with the same fields replays the receipt; the
  same id with different fields (`--dry-run` included) is refused. Use a new id per action.
- `stale incident` means the epoch moved on: kick again for the current incident.
- `revive` runs only with a recorded native session. A session gap's candidates are report-only:
  if you know the right one, `agentctl stop NAME` and `agentctl start NAME --profile P --resume ID`.
- `declare-done` succeeds only on the full verify's explicit YES.
- Exit 0 done and verified, 75 queued/running or still pending, 1 refused or failed.

### Asking for a judgement: `run-agent-prompt`

When no agent of the team is up to decide the next step, `run-agent-prompt` sends one prompt (from
`--prompt-file` or stdin) to the first harness that answers — Muse, then Claude, then Codex — with
no tools, and prints only the answer (exit 69 when every harness failed). Feed it the manifest,
never raw records, ask for one `team act` proposal, and run that action yourself:

```sh
{ echo "Given this recovery manifest, which single team act action should run next?"; \
  agentctl --registry R team manifest; } | run-agent-prompt --timeout 120
```

## The shepherd: the automated form of this protocol

`agentctl --registry R team shepherd` runs this protocol after a boot so an outside team's agents
do not stay dead when nobody is there to drive sections 1–3 by hand. It starts unconditionally from
its own boot unit in parallel with deterministic restore (no ordering on restore or Herdr; restore
never waits for it) and runs one incident per restore epoch — the kernel boot plus the user-manager
identity, the same epoch the restore replay guard uses. A rerun in the same epoch reports the
existing stamp and repeats no effect; a mid-session death is not its job (watch, `team kick` and the
driver own that).

Inside the incident, each round:

1. probes step 0: a team whose lead is live is judged read-only and never acted on, and only the
   shepherd's own team (its `--registry` registry) is ever acted on — teams named with `--config`
   are judged read-only in the same stamp;
2. gathers `team manifest` facts (never raw records), and, for its own team while its lead is down,
   runs `restore --dry-run`, reads the report, then the real `restore` — with `--force` only in the
   replayed-but-visibly-down case after a user-manager restart;
3. asks one stateless one-shot (`run-agent-prompt`: muse, then claude, then codex) to judge the
   manifest, and accepts at most one own-team `team act` proposal from its final JSON line — every
   effect goes through `team act`, so the driver validates, persists and re-verifies it.

It finishes only on the deterministic YES of section 4 (lead running, bridge healthy, handoff acked,
each an explicit pass; held/skipped never count), or on bounded give-up (20 rounds / 120 min by
default, early after two identical manifests) with escalation and a stamp under
`<state root>/shepherd/<boot-id>/`. If every harness is down while the checks pass, the stamp still
says YES with `judge=none`. Install its unit with
`agentctl --registry R install-units --dest ~/.config/systemd/user --shepherd`: installing writes
`agentctl-shepherd.service` only and never enables it — the operator reviews and enables it
deliberately.

## 4. Verify: a restore isn't done until all of these hold

`agentctl --registry R team verify` checks most of this list with evidence and exits 0 only
when no check failed; a held or skipped check is not a pass.

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
- **Revive refuses "record has no native conversation ID" for Muse lanes:** the lane's session was
  never recorded. While it is live, backfill it with
  `agentctl --registry R doctor --repair-native-sessions` (the session is observed from the process
  the record pins; several held sessions are never guessed between — bind one by hand with
  `agentctl bind-session NAME <id>`). New starts and promotes record the session themselves.
- **Revive blocked on a half-finished candidate whose Muse still lives:** just rerun
  `agentctl revive NAME`. When the retained candidate's harness is alive in its pane on the recorded
  session, revive pins and publishes it (the automated retire-stale-registration plus adopt proofs).
  It refuses — journal retained — when the live session differs or cannot be proved; only then fall
  back to the manual pins (`retire-stale-registration --expected-revive-sha256 … --replacement-pane …
  --replacement-session …`, then `adopt`). `--abandon-candidate` stays refused while a harness runs
  for the candidate.
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
