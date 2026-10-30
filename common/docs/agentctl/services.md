# Long-running agentctl services: snapshots and refresh

`agentctl services` keeps a team's long-running agentctl processes on a known,
compiled build, and moves them to a newer one safely.

## The problem

Short agentctl commands can run from a source checkout through any launcher:
each call starts, does its work, and exits. Three kinds of agentctl process
instead run for days as systemd user units:

- a **chat bridge**: `agentctl chat run --bridge-state DIR`;
- the **agent watcher**: `agentctl watch-agents`;
- a **headless agent's wrapper**: `agentctl run-headless NAME`.

Each loads its binary once and is restarted by systemd after a crash, so the
file it was started from must not change underneath it. A bridge also starts
helpers, the Slack plugin (found through `AGENTCTL_HOME`) and the outbound
helper (named in its `bridge.json`), which must come from the same build.

Moving such a service to a new build by hand takes a clean build from a
commit, a versioned copy of the binaries, a plugin home with its manifest, a
backup of the unit, a `systemd-run` that repeats every argument and environment
variable, a health check, and rollback notes. `agentctl services` does those
steps, and nothing else:

- no launcher and no rebuild on call: services run an ordinary release binary;
- no cache: every snapshot is a fresh `cargo build --release --locked` of a
  clean `git archive`;
- one service at a time, each checked before the next is touched.

## Dependencies

A systemd user manager (`systemctl --user`, `systemd-run --user`), Git, `tar`,
and a Rust toolchain with the build's dependencies already fetched (`cargo
build --locked` runs offline when the registry cache holds them). The services
must be systemd user units of the user running the command.

## The flow

```sh
agentctl --registry ~/team/.agentctl services list
agentctl services build --from ~/team/agent-utils
agentctl --registry ~/team/.agentctl services refresh --dry-run
agentctl --registry ~/team/.agentctl services refresh
agentctl --registry ~/team/.agentctl services list
```

Every command takes `--root DIR` (default `$XDG_STATE_HOME/agentctl/services`,
else `~/.local/state/agentctl/services`), which holds two collections, each in
its own directory:

```text
ROOT/snapshots/COMMIT12/          one compiled snapshot per commit
ROOT/refreshes/STAMP-COMMIT12/    one directory per refresh run
ROOT/refresh.lock                 held while a refresh runs
```

`--systemctl` and `--systemd-run` name other executables for those two
programs; they are always run with `--user`.

## Which units are services

A unit is a service of the registry when its single `ExecStart=` command runs
an agentctl binary, either as the program or as the command of a `sh -c`
script (after an optional `exec` and variable assignments), and:

- its `--registry` (default `.agentctl`, relative to the unit's
  `WorkingDirectory=`, which defaults to the user's home) is the same directory
  as this command's `--registry`;
- its subcommand is `chat run` with `--bridge-state`, `watch-agents`, or
  `run-headless NAME`.

An agentctl binary is a file named `agentctl` or `agentctl-SUFFIX` (a pinned
copy such as `agentctl-92f81d81`); the Slack helpers, which share the prefix,
are not. Arguments are read with agentctl's own option tables, so an option's
value is never mistaken for the subcommand. A `sh -c` script that expands
variables or commands, or that runs something after agentctl, cannot be
rewritten safely; `list` names such units as unreadable and `refresh` leaves
them alone.

## services list

For each service: the unit, its kind, whether it is active and its main
process, whether it is transient (made with `systemd-run`) or a unit file, the
binary and the commit it prints on `--version`, the process running now (called
out when it is not the unit's binary: replaced on disk, or the unit changed
without a restart), the agent of a headless wrapper, the plugin home
(`AGENTCTL_HOME`, else `~/.agentctl`), the outbound helper from `bridge.json`,
the state directories (the bridge state, the watcher's `--state-dir` or its
default, the headless agent's registry directory), the log redirection of a
`sh -c` script, and how it compares with the newest snapshot:

- **on the newest snapshot**: it runs the snapshot's `bin/agentctl` and, for a
  bridge, `AGENTCTL_HOME` is the snapshot's plugin home;
- **same commit, not run from it**: a hand-built copy of the same commit;
- **not on the newest snapshot**: `refresh` would move it.

It also says when a bridge's Slack plugin or outbound helper differs from the
snapshot's (by SHA-256). `--json` prints the whole report (schema
`agentctl-services-list/v1`). `list` reads systemd, `/proc`, and files, and
runs each service binary's `--version`; it changes nothing.

## services build

```sh
agentctl services build --from ~/team/agent-utils
agentctl services build --from ~/team/agent-utils --revision origin/main
```

1. Resolves `--revision` (default `HEAD`) in the checkout given by `--from`
   (default `$AGENT_UTILS_CHECKOUT`, else the checkout containing the current
   directory) to a full commit. A complete snapshot of that commit is reused
   (`"built": false`).
2. Extracts that commit with `git archive | tar -x` into a fresh directory
   under `--work-dir` (default: the system temporary directory). Uncommitted
   edits and untracked files never reach a snapshot.
3. Runs `cargo build --release --locked -p agentctl -p chat-subscription-slack
   --bins` there, with a fresh target directory, `CARGO_INCREMENTAL=0`, and
   `AGENTCTL_SOURCE_COMMIT` set, so the binary prints its commit on
   `--version`. Cargo's progress goes to standard error.
4. Stages the snapshot under a dot name: `bin/agentctl` and
   `bin/agentctl-slack-outbound` (mode 0500), and a plugin home
   `home/plugins/slack/` with `agentctl-slack-plugin` (0500) and the package's
   `manifest.json` (0400), in private directories.
5. Checks it: the staged agentctl must print the commit, and plugin discovery
   must accept the plugin home.
6. Writes `snapshot.json` (schema `agentctl-services-snapshot/v1`: commit,
   build time, source checkout and revision, cargo version, and each file's
   SHA-256 and size), then renames the directory into place.

A failure removes the staged directory and exits 75; the temporary directory
is removed unless `--keep-work`. A snapshot is never changed after it is
written, so a running service's binary is never replaced under it. Remove old
snapshots by hand once no service runs them (`services list` shows which do).

## services refresh

```sh
agentctl --registry ~/team/.agentctl services refresh --dry-run
agentctl --registry ~/team/.agentctl services refresh
agentctl --registry ~/team/.agentctl services refresh --only chat-bridge --snapshot 1a2b3c4d
```

It moves each selected service (`--only UNIT`, repeatable; default all) onto
the snapshot (`--snapshot COMMIT`, at least 7 characters; default the snapshot of the
newest commit, by commit time). A service already on it is left alone unless `--force`; one that is not
active is skipped. Watchers go first, then bridges, then headless wrappers, and
the first failure stops the run: later services are reported as not attempted.

**What changes.** Only the agentctl binary, and for a bridge `AGENTCTL_HOME`,
which points at the snapshot's plugin home so the bridge starts the matching
Slack plugin. Every other argument, environment variable, working directory,
restart policy, and log redirection stays as it was. The outbound helper is
part of the bridge's own configuration (`bridge.json`, with checksums of its
own), so refresh leaves it; `list` reports when it differs from the snapshot's.

**Before a unit changes**, the run directory `ROOT/refreshes/STAMP-COMMIT12/`
gets `UNIT.before` (its `systemctl cat`) and the unit's rollback commands,
appended to `ROLLBACK.sh`. `report.json` is written at the end.

**A transient unit** is made again by `systemd-run` from its own definition:
each `[Unit]` and `[Service]` line becomes a `-p` property (the description
`--description`), with the new command after `--`. Before the real unit is
touched, the same properties start a check unit
(`agentctl-services-check-PID-UNIT`) whose command is a shell that only
sleeps, given the planned command line as its arguments, and which leaves out
dependency keys such as `After=` and `Wants=`. It must read back as planned
and is stopped at once. That proves systemd-run accepts every property and
quotes every word of the command as planned, so neither the change nor the
rollback can fail on one; a check unit left behind by a killed refresh is
stopped by the next. Then the unit is stopped and `reset-failed`; once systemd
has unloaded it, it is made again, and its definition must read back as
planned. A unit that stays loaded after it stops (another unit or a client
still refers to it) is not replayed: it is started again from its own
definition, which is the rollback.

Refused, to be refreshed by hand: a transient unit with drop-ins (`systemctl
cat` would merge them into the replay, and the new unit would load them again),
a unit with `ExecStartPre=` or other extra commands, an `ExecStart=` prefix, a
program named without a path, and a bridge whose `AGENTCTL_HOME` could come
from elsewhere (`EnvironmentFile=`, `UnsetEnvironment=`, `PassEnvironment=`, or
an assignment in its script).

**A unit file** gets the drop-in `DROP_IN_DIR/UNIT.d/zz-agentctl-services.conf`
(`--drop-in-dir`, default `$XDG_CONFIG_HOME/systemd/user`), which resets
`ExecStart=` and sets the new command (and `AGENTCTL_HOME`). After `systemctl
--user daemon-reload`, the unit's effective `ExecStart=` must be the planned
one (another drop-in sorted later would win), then it is restarted. The unit
file itself is never edited.

**Health.** Each service must pass the same check before anything changes
(without the "since the restart" conditions); a service that does not is left
alone and the run stops, so a problem it already had (a bridge whose target
agent is gone, say) is never blamed on the new build or rolled back over.

After the change, within `--health-timeout` (default 120 seconds; a watcher
gets at least its `--pass-budget-seconds` plus 90 seconds), and for at least
30 seconds, the unit must be active with one main process that runs the new
binary (compared by canonical path; for a script without `exec`, a child of the
shell that does); a changed main process means a crash and restart. Then, by
kind:

- **bridge**: the new binary's `chat health --json` (killed after 60 seconds)
  must not say DOWN and must name that process as the service; the process must
  run with the snapshot's `AGENTCTL_HOME` (read from `/proc/PID/environ`); and
  its event stream must have delivered something (a batch or a heartbeat) after
  the restart;
- **watcher**: `agent-watch.json` in its state directory must show a pass
  (`checked_at`) completed after the restart;
- **headless wrapper**: its control socket must answer `status` from that
  process.

A headless agent in a turn is waited for (`--headless-wait`, default 600
seconds), so the restart never cuts a turn short, and skipped if it stays busy.

**Rollback.** When the change or the check fails, the service is put back at
once: a transient unit is stopped and made again from its original definition
(which must read back as the backup); a drop-in is removed, or its previous
contents restored, followed by a reload and a restart. The old service is then
health-checked the same way.

**A rollback pairs a binary with its state.** Rolling back restores the old
binary, not the state the old binary last saw. Configuration files
(`profiles.json`, `restore.json`, the bridge's `chat.json` and `bridge.json`,
plugin manifests) take a key from a newer build with a warning, but the records
a service writes as it works (a bridge's request and reply records, the agent
registry's records, the watcher's state) are read strictly, and a record the
new build has already written can stop the old binary. Before refreshing a
service you may need to roll back by hand, copy its state directory (`cp -a`)
beside the old binary, and roll back to the two together.

**Output** is one line per service, `UNIT (KIND): OLD_COMMIT OLD_BINARY ->
NEW_COMMIT NEW_BINARY: healthy: DETAIL`, or what failed and how the rollback
went, then the run directory. `--dry-run` prints the plan and its commands and
writes nothing. `--json` prints schema `agentctl-services-refresh/v1`.

**Interruption.** During a refresh, SIGINT, SIGTERM and SIGHUP do not stop it
halfway: the service in hand is finished or rolled back, the rest are not
attempted, and the exit status is 1. Child processes run in their own process
group, so an interrupt typed at the terminal never reaches a `systemctl` in the
middle of a change. Nothing can catch SIGKILL: run refresh in a terminal or a
pane, not under a tool's timeout that kills it.

Exit status: 0 when every selected service runs the snapshot and passed its
check (or already ran it); 1 when a service was refused, was unhealthy
beforehand, failed before anything changed, or failed and was rolled back, or
when the run was interrupted; 4 when a rollback failed too,
so a service may be down: run the commands in the run's `ROLLBACK.sh`. Errors
before any service is touched (no snapshot, another refresh running) exit 75.

## Trying it without touching real services

Use a scratch registry and throwaway units:

```sh
mkdir -m 700 -p /tmp/try/registry /tmp/try/watch
systemd-run --user --unit=try-watch --description="trial watcher" \
  -p WorkingDirectory=/tmp/try \
  /bin/sh -c "exec /path/to/old/agentctl --registry /tmp/try/registry watch-agents --state-dir /tmp/try/watch 2>>/tmp/try/watch.log"
agentctl --registry /tmp/try/registry services --root /tmp/try/root build --from ~/team/agent-utils
agentctl --registry /tmp/try/registry services --root /tmp/try/root refresh
systemctl --user stop try-watch
```
