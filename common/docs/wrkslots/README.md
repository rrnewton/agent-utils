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
boundary. The same-user processes can bypass any such convention. Removal accepts the flag as
optional provenance. Recovery requires it only when starting a new direct cleanup of an
unregistered validation path; resuming the durable journal never depends on the original
coordinator.

An assigned agent can run `create` with its own live `--owner-pid` and the live
`--coordinator-pid` it was given, including when they run in separate terminal panes. The tool
records and rechecks both exact process identities. Only an invoking coordinator can create an
unbound slot or assign another owner, which must be its descendant.

The owner PID must represent that one agent's lifetime. Do not bind several logical agents to a
shared multiplexing supervisor, coordinator, or application-server PID: process ancestry cannot
distinguish those agents, and the shared live generation prevents per-agent liveness and reclaim.
Separate coordinator and owner processes must also be visible in the same Linux PID namespace.

`create` uses an existing source repository and its configured `origin` remote by default. Use
`--remote NAME=REMOTE` when a checkout should use a different configured remote. Add
`--remote-url NAME=URL` when the caller must verify that remote's exact fetch URL; otherwise
wrkslots records the configured URL. Repository paths must be relative: use an ordinary path inside
the project root, or path components of the form `../NAME` for one direct sibling repository. This
is a normalized-path rule rather than a byte-for-byte spelling requirement, but every other raw
`..` traversal, every absolute path, and every path with a symlink component is refused. Accepted
paths are stored in normalized relative form. It creates a new linked worktree and local branch; it
never reclaims another slot to satisfy an allocation.

To register a live slot that already exists on disk but has no active row, first inspect the exact
arguments and then apply the registration with all ownership fields:

```sh
wrkslots import-existing slot01 --help
wrkslots import-existing slot01 --slot-type agent --coordinator-authorized \
  --agent codex-1 --task task-123 --purpose "continue task-123" \
  --repo product=../product --apply --verified-live \
  --owner-pid "$OWNER_PID" --coordinator-pid "$COORDINATOR_PID"
```

`status` is read-only: it returns the complete readable roster and reports every observable
registry/storage disagreement as a typed inconsistency instead of withholding the roster. It can
inspect Git registrations only for source repositories named by readable rows; a repository absent
from the registry is not discoverable until a command such as `create --repo` supplies it. `create`
refuses a name, path, or Git registration that overlaps its requested slot, except for the exact
selected source worktree when that source contains the configured managed root. It may create at a
distinct target while printing unrelated inconsistencies it retained. Other lifecycle mutations remain strict by
default while a managed directory contains worktrees that have no wrkslots record. During a
deliberate migration, place the global `--allow-existing-unregistered-worktrees` flag before such a
command. The command retains those directories and acts only on registered slots; use `wrkslots
audit --format json` to inventory what still needs evidence-based import.

Remote salvage remains the default and preferred preservation path. If a recorded remote refuses
an agent slot's salvage ref, an operator may rerun that one removal with an existing, absolute
archive directory outside the project:

```sh
wrkslots remove slot01 --coordinator-authorized \
  --coordinator-pid "$CURRENT_COORDINATOR_PID" --expected-generation 1 \
  --salvage-archive-root "$HOME/temp/agent_checkouts"
```

This opt-in fallback writes a separate self-contained Git bundle and schema-1 receipt for each
affected top-level or initialized nested repository. Wrkslots verifies the SHA-256, exact archive
ref, complete restoration into an empty bare repository, and full object connectivity before the
ordinary path fence may remove anything. A failed push is recorded as `archived-local`, never as
remote `salvaged`; omission of the option leaves the slot untouched.

For initialized nested repositories, a later source-checkout switch between recognized GitHub
HTTPS and SSH spellings does not block cleanup when both URLs still name the same owner and
repository. Wrkslots salvages through the slot's own checked URL. Different hosts, owners, or
repository names, and non-GitHub URL spelling changes, still refuse.

`import-existing --from-state-file worktree-state.json --source-host-id ID` admits a slot whose
owner has already exited from an exact version 3 source row. It preserves the row and source-file
digest, starts a fresh heartbeat time-to-live, and records the complete candidate ACTIVE row before
publication so `recover` can finish after the original participant disappears. Rows without an
owner sidecar refuse rather than turning unavailable ownership into permission to delete.

An unregistered validation checkout or `validate-cargo-*` directory with no recoverable owner is
not imported with invented ownership. Destructive recovery of an ordinary linked Git worktree is
disabled: wrkslots cannot atomically exclude both its checkout and same-UID-writable Git
administration, so the checkout is preserved for inspection. Use `import-existing` only for a
demonstrably live owner. The separate `validate-cargo-*` recovery route accepts an exact terminal
run record, or an explanation when no record survives, and records the exact path and filesystem
identity before bounded cleanup. Ordinary `status` reports every remaining unregistered directory
as an inconsistency and still returns the readable active roster; it does not authorize cleanup or
make an inconsistent row healthy.

Agent drift has separate bounded recovery commands. `recover-absent-agent-row` accepts one exact
ACTIVE identity, publishes and reads back every recorded commit, removes only matching stale Git
registrations, and archives before removing the row. `recover-ownerless-agent-worktree` accepts one
exact unregistered worktree and its complete Git identity, salvages authored and nested-repository
work without assigning an owner, task, or handoff, then fences and removes it. A present HANDOFF.md
requires its exact SHA-256 after the coordinator reads it; that content is rechecked and preserved
with the worktree. `recover-ownerless-agent-cache` relocates only its one explicitly supported cache
tree outside the managed slot root; it is not an exemption for arbitrary directories.

Agent handoffs can live outside their checkouts. `write-handoff` copies bounded UTF-8 from an exact
live owner into a generation-bound control sidecar; it never exempts checkout dirt from `finish`.
`read-handoff` prints the exact contents and durably enqueues that slot generation without removing
anything. `retirement-queue` inspects pending items, and `retire-pending --limit N` attempts each
through the unchanged ordinary remove state machine, retaining every refusal for a later retry.
Existing direct-child `HANDOFF.md` files are copied only when read, must agree with any sidecar, and
remain in place until the slot is successfully archived and removed.

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
repeatedly scanning every process on a shared host. Five tests require host-visible process
evidence and run outside that namespace. Coverage and assertions are identical.

See [USER_GUIDE.md](USER_GUIDE.md) and [RELATED_WORK.md](RELATED_WORK.md).
