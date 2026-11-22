# Recovering an agent team

Use this guide when an agentctl team has stopped working: the coordinator doesn't answer, lanes
are missing from Herdr, or the chat bridge restart-loops. A **team** is one registry directory
(`<team>/.agentctl`) holding agent records, launch profiles and `restore.json`. Its **lead** is the
designated coordinator. Its **units** are its systemd user services (Herdr server, chat bridge,
watcher). The [`team-restore` skill](../../../skills/team-restore/SKILL.md) holds the same protocol
for an agent to follow.

Every command below takes the registry as `--registry R`. Run them from the host's own PID
namespace (the `team` commands refuse inside a team's namespace), with the Rust edition of
`agentctl` (`common/bin/agentctl` from a checkout runs it by default). The installed `--help` is the
authority for flags and exit codes.

```sh
R=/path/to/team/.agentctl
```

## What kills a team

| Event | What dies | What recovery has to do |
|---|---|---|
| Host reboot | Everything. | The boot unit `agent-restore.service` (from `install-units`) runs `agentctl restore --boot`. Without it, run restore by hand. |
| User-manager restart (`user@UID.service` restarted or killed, or the last login ended without linger) | Every user unit (Herdr server, bridges, watcher), so every agent in Herdr too. The kernel boot id stays the same. | Restore again. Current builds treat a new user manager as a new restore epoch (#1405). Older builds think this boot was already restored and need `--force`. |
| Herdr server dies | Every agent pane. Units keep running. | Restore. Bridges restart-loop on `agent_not_found` until their target agent is back, then recover by themselves. |
| Bridge failure | Only the bridge. | See "Known failures" below. Agents are fine. |

## 0. One restorer per team

Two restorers acting on one team make things worse: they revive the same lane twice, or one
moves state the other is reading. Before you change anything:

- Ask the lead (`agentctl --registry R send <lead> ...`). If it answers, it owns recovery: give it
  what you found and stop.
- Check whether the boot unit is still running (`systemctl --user status agent-restore.service`)
  and whether a team driver is active (its state is in the `team kick` report, below).
- If you help another team, say what you are about to do before you do it.

## 1. Look (read-only)

```sh
agentctl --registry "$R" list                         # what should run, and what is live
agentctl --registry "$R" team manifest > manifest.json
systemctl --user list-units 'agent*' --all            # note anything in "activating (auto-restart)"
```

The manifest (schema `agentctl-team-manifest/v1`) is a redacted summary of the recovery facts:
the desired set, each agent's liveness, profile and effort, `record_gaps` (records missing a
profile, bypass flag, declared effort or recorded session), queued prompts, unit state, `lost[]`
work that no restore can bring back, and an embedded `team verify` result. It never contains
record tokens or launch arguments, so it is safe to share.

## 2. Dry run

```sh
agentctl --registry "$R" restore --dry-run > restore-dry.json
```

Read it before running anything:

- `desired_state_source` should be `registry-derived` (every record that was running at shutdown).
  `pinned-list` means `restore.json` has an `agents` key, which parks every agent not on it. Back the
  file up, remove the key, and dry-run again. Put agents that should stay down in `exclude` instead.
- `action: already-restored` with `replayed: true` while agents are visibly down means this epoch
  was already restored once. Add `--force` to the real run (the user-manager restart case on older
  builds). `--force` only skips the replay. It never overrides a refusal.
- `complete: false` with `in_progress: true` is a killed attempt's partial report. A rerun resumes
  from it. Don't delete it.
- Read `refused` and each agent's `reason` now.

## 3. Run it

```sh
agentctl --registry "$R" restore [--force] > restore.json; echo "exit $?"
```

It can take minutes, so run it under `nohup` or with a long timeout. Exit 0 means done. 74 means
done except the lanes named in `refused_lanes`. 75 means incomplete: rerun it, and it resumes.
The report is also saved as `R/restore/reports/<boot-id>.json`. For several teams on one host, use
`agentctl restore-teams --config teams.json` (dry-run it first) instead of one `restore` per team.

Or, instead of steps 2 and 3, let the deterministic team driver run the whole recovery:

```sh
agentctl --registry "$R" team kick > kick.json; echo "exit $?"
```

The driver detects what is missing, restores, verifies, hands the remaining checklist to the lead
on a deadline, and escalates through the routes in `escalation.toml` if the lead doesn't answer.
Exit 75 means recovery is pending with a wake armed, not that it failed. It runs one driver per team
under a lock, so firing it twice is safe. Don't also run `restore` by hand while it works.

## 4. Fix what's left with `team act`

`team act` queues one allowlisted action for the team driver, which checks it against a fresh
manifest, runs it, records a receipt, and verifies again. It never rewrites a record or binds a
session. The actions are `facts`, `revive`, `start-unit`, `reset-unit`, `drain`, `kill-pid`,
`declare-done` and `escalate`.

```sh
I=$(jq -r .state.incident kick.json)                  # the incident id (12 hex) from a kick report
agentctl --registry "$R" team act --incident "$I" --request-id r1-check --dry-run revive lane1
agentctl --registry "$R" team act --incident "$I" --request-id r1 revive lane1
agentctl --registry "$R" team act --incident "$I" --request-id r2 reset-unit agent-watch.service  # a restore.json unit
agentctl --registry "$R" team act --incident "$I" --request-id r3 declare-done
```

- `--request-id` is an idempotence key: the same id with the same fields returns the recorded
  receipt without running again, and the same id with different fields is refused. Use a new id
  for each new action (`--dry-run` counts as a different field).
- A `stale incident` refusal means the epoch moved on: kick again and use the new incident.
- `revive` needs a recorded native session. A `record_gaps` session entry lists candidate sessions
  for information only. Recovery never picks one. If you know which is right, run
  `agentctl stop NAME`, then `agentctl start NAME --profile P --resume ID`.
- `declare-done` succeeds only when the full verify says YES.
- Exit 0 means done and verified. 75 means queued, running, or done with recovery still pending.
  1 means refused or failed, with the reason printed.

When no agent is up to judge what to do next, `run-agent-prompt` asks one question of the first
harness that answers (Muse, then Claude, then Codex) and prints only the answer. It runs with no
tools, so you still run any action it proposes:

```sh
{ echo "This agent team is recovering from an outage. Given this manifest, which single team act action should run next, and why?"; cat manifest.json; } \
  | run-agent-prompt --timeout 120
```

Give it the manifest, never raw records. Exit 69 means every harness failed.

## 5. Verify: it isn't done until this passes

```sh
agentctl --registry "$R" team verify > verify.json; echo "exit $?"
```

`team verify` exits 0 only when no check failed: `desired-live` (every desired agent live with its
declared profile), `units-bridges` (units active, every bridge healthy with its target present),
`queued-not-archived` (no prompt queued before the outage was archived undelivered),
`watcher-dead-agents`, `pane-at-prompt`, and `coordinator-ack` (the lead processed its outage
notice). A check reported as held or skipped is not a pass. Then check the parts no command
proves:

- Each agent that should run at a raised reasoning effort shows it in its status bar.
- `agentctl --registry "$R" chat health --bridge NAME` says OK for each bridge.
- The lead answers a message, and has told its lanes to re-check work that was running when the
  outage hit.

## Known failures

| Symptom | Fix |
|---|---|
| Bridge restart-loops on `agent_not_found` | Its target agent isn't in Herdr. Restore that agent. The bridge recovers on its next restart. |
| Bridge stops on "unrecoverable stream gap" | It was down longer than the event stream keeps events. Stop the unit, move its state directory aside (`bridge.old-gap-<date>`), run `agentctl chat init --config OLD/bridge.json --from OLD`, then read the channel by hand for messages the gap swallowed. |
| Revive refuses "record has no native conversation ID" | The session was never recorded. Before the next outage, run `agentctl --registry "$R" doctor --repair-native-sessions` while lanes are live. |
| Revive refuses "death unverifiable" or "pane/cwd changed" | `agentctl stop NAME`, then `agentctl start NAME --profile P --resume SESSION`. |
| A lane came back at a lower effort | `agentctl restart NAME --when-idle` relaunches it from its profile on the same session, at an idle turn boundary. |

## Don't

- Don't run two restorers on one team: no manual `restore` while a driver or boot unit runs, and
  never restore another team's agents while its lead is alive.
- Don't hand-edit the registry (`agents/<name>/agent.json`, the driver's state, or queue files),
  and don't pin `restore.json` from the live list (`restore --write-agents`) as routine. A pinned
  list goes stale and parks lanes at the next outage.
- Don't delete restore reports (partial ones included), bridge state or archived inboxes. Move
  them aside.
- Don't kill processes with a raw `kill`, `pkill` or `killall`. Use `agentctl kill-pid PID
  --expect-cmdline TEXT --expect-start TICKS`, and never signal the user manager
  (`systemd --user`): that stops every team on the host.
- Don't raise the boot unit's timeout to fit a big team. Restore resumes from its partial report,
  so an attempt that makes no progress is a hung lane to diagnose.

## When it still fails: what to collect

Send these to whoever maintains your agent-utils install:

- The build: `agentctl capabilities` (its `build` block) and the agent-utils commit you run.
- `restore-dry.json`, `restore.json` (or `R/restore/reports/<boot-id>.json`), `manifest.json`,
  `verify.json`, and `kick.json` or the driver's `state.json` (under
  `~/.local/state/agentctl/team-drivers/<key>/`).
- `journalctl --user -b -u agent-restore.service` (plus `agent-restore-teams.service`,
  `agentctl-team@*` and your bridge units), and `systemctl --user list-units 'agent*' --all`.
- `agentctl --registry "$R" doctor`, `agentctl --registry "$R" list`, `herdr agent list`, and
  `agentctl chat health --bridge NAME --json` for each bridge.

The manifest, `list` and `status` leave out tokens and launch argument values. Don't send raw
`agent.json` records or `status --show-argv` output: they hold tokens.

## Before the next outage

- Install the boot units: `agentctl --registry "$R" install-units --dest ~/.config/systemd/user`
  writes `herdr-server.service`, `agent-restore.service` and `agent-watch.service`. For several teams,
  add `--teams-config teams.json`. It never enables anything: run the `systemctl` commands it prints.
- Before a planned reboot, `agentctl --registry "$R" shutdown` exits every agent at an idle turn
  boundary and records sessions and panes for restore. Run it from a plain shell, not from an
  agent it would exit.
- Keep `restore.json` without an `agents` key, and record missing native sessions with
  `doctor --repair-native-sessions` while lanes are live.
