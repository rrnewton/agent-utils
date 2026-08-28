# wrkslots

`wrkslots` manages Git worktree slots used by coordinators and coding agents. It records who owns
each slot, keeps machine-sharded durable state, refuses ambiguous deletion, and resumes interrupted
operations from journals.

The tool is intentionally conservative. A heartbeat timeout or old filesystem timestamp is useful
diagnosis, but neither authorizes deletion. Physical removal requires the registered coordinator,
the configured running check, process-generation evidence, clean and published Git state, and
absence of live process use.

## Install

```sh
python3 -m pip install ./py/wrkslots
```

Python 3.10 or newer and Git are required. Linux `/proc` is required for the complete live-process
safety checks.

## Quick start

Run the built-in tutorial first:

```sh
wrkslots quickstart
```

A normal lifecycle is:

```sh
wrkslots init . --liveness-command tools/agent-liveness.py

wrkslots create slot01 \
  --agent codex-1 --task task-123 --purpose "fix parser" \
  --coordinator-pid "$COORDINATOR_PID" --owner-pid "$OWNER_PID" \
  --repo product=product --branch product=codex/fix-parser

wrkslots heartbeat slot01 \
  --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1

wrkslots finish slot01 \
  --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1 \
  --validation "make test: pass"

wrkslots remove slot01 \
  --coordinator-pid "$COORDINATOR_PID" --expected-generation 1
```

`create` uses an existing source repository and its configured `origin` remote by default. Use
`--remote NAME=REMOTE` when a checkout should use a different configured remote. Add
`--remote-url NAME=URL` when the caller must verify that remote's exact fetch URL; otherwise
wrkslots records the configured URL. It creates a new linked worktree and local branch; it never
reclaims another slot to satisfy an allocation.

If a command reports an interrupted journal, preserve the paths and run:

```sh
wrkslots recover --coordinator-pid "$COORDINATOR_PID"
```

## Development and stress testing

From this directory:

```sh
python3 -m pytest -q
python3 tests/e2e_stress.py --seed 1 --workers 8 --seconds 20
```

The stress harness creates only temporary local repositories and bare remotes. It uses real child
process trees, Git worktrees, concurrent CLI commands, controlled process death, and invariant
checks. A failure prints the seed and retains a replay trace. Wall time also includes the real host
process-use scans; compressed leases do not bypass or shorten those checks.

See [USER_GUIDE.md](USER_GUIDE.md) for the command model, durable state, recovery behavior, and
test scenarios.
