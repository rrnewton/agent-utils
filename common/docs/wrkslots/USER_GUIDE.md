# wrkslots user guide

## What a slot contains

A slot is one durable registry row plus one directory under the configured worktrees directory. A
slot can contain one or more linked Git worktrees. Each checkout records its source repository,
branch, starting commit, configured remote name, remote identity, landed ref, current commit, and
the remote refs that contain that commit at handoff.

The source repositories stay outside the managed worktrees directory. `wrkslots` treats the managed
directory as opaque: do not move its entries or edit `ACTIVE.*`, `ARCHIVED.*`, or journal files by
hand.

## Normal lifecycle

1. `init` creates the configuration and empty machine shard. It registers the project command used
   to determine whether an agent is dead, alive, or unverifiable.
2. `create` verifies each source repository and configured remote, creates a new branch and linked
   worktree, and durably records the coordinator and optional owner process generations.
3. `heartbeat` refreshes diagnosis data for the exact recorded owner generation.
4. `finish` runs while the owner is still alive. It requires clean Git state, fetches the configured
   remote, proves that every checkout commit is published and contained by the configured landed
   ref, then records validation evidence and the coordinator continuation. It does not delete.
5. `remove` is coordinator-only. It requires the registered running check to report dead, the owner
   process generation to be absent, and the live-process and Git safety checks to succeed.
6. `recover` resumes an interrupted create or removal from its durable journal.

Run `wrkslots quickstart` for copyable commands and `wrkslots COMMAND --help` for the exact inputs
and effects of one operation.

A configuration written by an older build makes normal commands refuse. `init --repair` can add
compatible configuration fields, upgrade the exact empty schema-1 active and archive files, and
replace the repository command symlink written before packaging. It refuses populated, malformed,
or other unsupported state rather than reporting an empty or partial slot list.

## Registered running command

`init --liveness-command PATH` records an executable path relative to the project root. During
removal, wrkslots invokes it with the recorded agent name as its only positional argument and
provides `WRKSLOTS_PROJECT_ROOT`, `WRKSLOTS_SLOT`, `WRKSLOTS_AGENT`, `WRKSLOTS_MACHINE`, generation,
owner PID/start/boot/cgroup fields, and other recorded identity fields in the environment.

The exit status is authority:

- `0`: the registered mechanism verified the agent is dead;
- `1`: the agent is alive;
- `2`: the mechanism cannot determine the answer;
- anything else: the check failed and removal refuses.

Heartbeat expiry, directory mtimes, and apparent inactivity never replace this result. They are
diagnostic signals for a coordinator or human.

## Git remotes

For each `--repo NAME=PATH`, `create` defaults to the repository's configured `origin`. Supply
`--remote NAME=REMOTE` to choose another configured remote. The remote must have exactly one fetch
URL. Supply `--remote-url NAME=URL` when the caller must verify that exact URL during provisioning;
otherwise wrkslots reads the configured URL. In both cases it records a SHA-256 identity and
requires the URL to remain unchanged through handoff and removal.

## Recovery

Every operation that can leave multiple durable effects writes a journal first and updates it after
each completed step. A crash therefore produces a named recovery state rather than an inferred one.
Ordinary mutation commands refuse while a journal exists.

Recovery validates the journal's machine, slot, process generation, paths, Git registrations, and
registry rows before continuing. Repeating recovery is safe. A mismatch preserves all remaining
paths for inspection.

## Test scenarios

The source package includes deterministic and stress tests under `tests/`:

- happy path: coordinator creates, assigns an agent process tree, work is committed and pushed,
  handoff is recorded, and the coordinator removes the slot;
- forgotten cleanup: the agent exits and the lease ages while the clean published slot remains;
- agent death with dirty files or unpublished commits: cleanup refuses and preserves the work;
- concurrent coordinators and agents issuing conflicting lifecycle commands;
- process use through cwd, open descriptors, mapped files, executables, and descendants;
- real process killing at journal transitions followed by recovery;
- cross-shard duplicate and unrelated-slot preservation checks.

The default test run uses compressed durations. The standalone stress runner accepts a seed, worker
count, and duration and writes a replayable JSONL trace when an invariant fails.

The mock processes use the same observable parent/child shape as the coding-agent sessions on the
development host: a coordinator starts a launcher, the launcher starts the agent engine, and the
engine starts command processes. The harness registers the engine as owner, invokes owner-only
commands from descendants of that engine, and uses real signals to kill either the engine or its
whole process tree.

## Filesystem timestamps

A directory mtime changes when entries immediately inside that directory are added, removed, or
renamed. It is not the newest mtime of every descendant, including on btrfs. Determining the exact
newest mtime in an arbitrary existing tree therefore requires walking the tree unless another
component has maintained an index or change log. Wrkslots treats timestamps only as diagnosis and
does not use them as removal authority.
