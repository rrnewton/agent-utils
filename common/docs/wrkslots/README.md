# wrkslots

`wrkslots` is the single lifecycle interface for Git worktrees used by coordinators, coding agents,
and disposable validation. It records typed ownership and renewal evidence in an append-only,
hash-linked history, salvages agent work before reclaim, and lets any later participant finish an
operation whose original participant disappeared.

It is deliberately fail-closed. Reclaim requires the recorded time-to-live to expire, the project
liveness command to report dead, the exact owner process identity to be dead, and process, mount,
Git, and path checks to agree. Unavailable evidence refuses. `agent` slots publish dirty and
unpushed work, including initialized Git submodules, before deletion; `validate` slots skip salvage because their evidence must live
outside the disposable checkout. `remove --validate-complete` lets the exact owner clean up a
completed validation slot immediately, or lets a later participant skip only the heartbeat wait
after owner death.

## Install

```sh
python3 -m pip install ./py/wrkslots
```

Python 3.10 or newer, Git, and Linux `/proc` are required.

## Quick start

```sh
wrkslots init . --worktrees-dir worktrees/slots \
  --liveness-command tools/agent-liveness.py

wrkslots create slot01 --slot-type agent --coordinator-authorized \
  --agent codex-1 --task task-123 --purpose "fix parser" \
  --coordinator-pid "$COORDINATOR_PID" --owner-pid "$OWNER_PID" \
  --repo product=product --branch product=codex/fix-parser

wrkslots heartbeat slot01 \
  --agent codex-1 --owner-pid "$OWNER_PID" --expected-generation 1

wrkslots remove slot01 \
  --coordinator-pid "$CURRENT_COORDINATOR_PID" --expected-generation 1
```

Creation requires `--coordinator-authorized` as a readable reminder, not as a claimed permission
boundary. The same-user processes can bypass any such convention. Removal and recovery accept that
flag only as optional provenance so a departed coordinator cannot strand an operation.

The strict default refuses while a managed directory contains worktrees that have no wrkslots
record. During a deliberate migration, place the global
`--allow-existing-unregistered-worktrees` flag before the command. The command retains those
directories and acts only on registered slots; use `wrkslots audit --format json` to inventory what
still needs evidence-based import.

`import-existing --from-state-file worktree-state.json --source-host-id ID` admits a slot whose
owner has already exited from an exact version 3 source row. It preserves the row and source-file
digest, starts a fresh heartbeat time-to-live, and records the complete candidate ACTIVE row before
publication so `recover` can finish after the original participant disappears. Rows without an
owner sidecar refuse rather than turning unavailable ownership into permission to delete.

If a command reports an interrupted operation, preserve the paths and run:

```sh
wrkslots recover --coordinator-pid "$CURRENT_COORDINATOR_PID"
```

Run `wrkslots quickstart`, `wrkslots COMMAND --help`, or `wrkslots --userguide` for the complete
contract.

For a periodic coordinator reminder, `wrkslots audit --gate` returns 1 when a slot is ready for
reclaim or an interrupted/unregistered slot needs attention, 2 when expired-slot evidence is
unavailable, and 0 when no action is currently indicated. It never removes anything.

## Development and stress testing

```sh
make test TEST_SUITE=python
python3 tests/e2e_stress.py --seed 1 --workers 8 --seconds 20
```

The lifecycle suite uses a PID namespace where possible to retain real process checks without
repeatedly scanning every process on a shared host. The four tests that require host-visible child
PIDs still run outside that namespace. Coverage and assertions are identical.

See [USER_GUIDE.md](USER_GUIDE.md) and [RELATED_WORK.md](RELATED_WORK.md).
