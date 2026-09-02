# wrkslots user guide

## What a slot contains

A slot is an append-only history plus a directory of linked Git worktrees. An `agent` slot holds
authored work and must be salvaged before reclaim. A `validate` slot is disposable by construction;
its logs and result records must live outside the checkout, so reclaim does not salvage it.

Use `worktrees/slots/<slot>/` for live agent slots and `worktrees/validate/<slot>/` for validation
slots. New-slot source repositories stay outside both directories. The recovery-only path may name
an existing source worktree inside managed storage solely to prove a stranded checkout's Git common
directory; it does not make that path valid for provisioning. `ACTIVE.*` and `ARCHIVED.*` are
readable compatibility views derived from `EVENTS.*`; do not edit any of them or a journal by hand.

Each checkout record carries its source repository, branch, starting commit, configured remote,
remote identity, landed ref, current commit, and remote containment evidence. Each slot records its
type, task and purpose, owner process identity, coordinator history, heartbeat time, and time-to-live.

## Normal lifecycle

1. `init` creates configuration, an empty machine history, the managed directories, and a
   project-local command symlink. It records the project command that distinguishes dead, alive,
   and unverifiable owners.
2. The coordinator runs `create --slot-type agent --coordinator-authorized` for authored work, or
   `create --slot-type validate --coordinator-authorized` for disposable validation. The flag is a
   reminder and recorded provenance, not a permission boundary between same-user processes.
3. The exact owner runs `heartbeat` to renew the slot while work continues.
4. An owner that has clean, published work may run `finish` to record validation and continuation
   evidence. A departed owner is not required to return: later reclaim can salvage from the recorded
   checkout state.
5. Any later participant may run `remove`. Reclaim proceeds only when the heartbeat time-to-live is
   expired, the registered running command reports dead, the exact owner process identity is dead,
   and process, mount, Git, and path checks agree. Unknown evidence refuses; it is not treated as
   free. `--validate-complete` lets the exact live owner remove its completed validation slot, or
   lets a later participant remove it after proven owner death without waiting out the heartbeat.
   Process-use, path, and Git checks still run.
6. Before removing an agent slot, `remove` publishes unpushed commits and tracked, untracked, and
   ignored files outside configured regenerable cache paths to the recorded remote. It records and
   rechecks the exact remote ref and commit before deletion. A validate slot skips this step.
7. `recover` lets any later participant complete an interrupted create or removal from the durable
   history. It is not tied to the participant that began the operation.

Run `wrkslots quickstart` for copyable commands and `wrkslots COMMAND --help` for exact effects and
inputs.

## Creation is coordinator-owned guidance

`create`, `register`, and `import-existing --apply` require both `--slot-type` and
`--coordinator-authorized`. Omitting either refuses before any worktree or lifecycle record changes
and tells the caller to ask the coordinator. This prevents accidental self-allocation; it does not
pretend that same-user processes have different operating-system permissions.

`--coordinator-authorized` on `remove` and `read-handoff` is optional provenance. It is required
when `recover` starts a new cleanup for an unregistered validation path, because that operation has
no ACTIVE row naming who allocated it. Resuming the durable journal does not require the original
flag or coordinator; otherwise a departed coordinator could strand an interrupted cleanup.

`status` always returns the complete readable roster and reports each observable directory or
Git-registration disagreement as a typed inconsistency. Git registrations are observable only for
source repositories named by readable rows; config does not contain a repository inventory.
`create --repo` supplies the requested repositories, so create also reports unrelated managed-root
registrations it discovers there. `create` refuses an overlap with the requested name, slot
path, checkout path, or Git registration. The exact selected source worktree is the sole ancestor
exception when it contains the configured managed root. Create may proceed at a distinct target
while printing every unrelated inconsistency it retained. Other lifecycle mutations refuse by default if either managed
directory contains a worktree without an active wrkslots record. During a deliberate migration,
put the global `--allow-existing-unregistered-worktrees` flag before such a command. The requested
operation then touches only registered slots and prints how many unregistered directories it
retained. It never uses that flag as permission to inspect, select, or remove one of them. Run
`wrkslots audit --format json` and import each live slot only after verifying its process evidence.

`wrkslots audit --gate` is the read-only coordinator reminder. It exits 1 for reclaimable,
interrupted, or unregistered slots; exits 2 when an expired slot cannot be classified because
evidence is unavailable; and exits 0 only when neither condition exists. The output names the
affected slots and the next command. It never converts an unknown result into permission to remove.

## Time-to-live and process evidence

`init --heartbeat-ttl-seconds SECONDS` records the default copied into every new slot. `heartbeat`
updates the durable renewal time only for the exact owner process generation. Expiry is one required
reclaim fact, never the whole decision.

The registered running command receives the agent name as its only positional argument and receives
`WRKSLOTS_PROJECT_ROOT`, `WRKSLOTS_SLOT`, `WRKSLOTS_AGENT`, `WRKSLOTS_MACHINE`, generation, and owner
identity fields in the environment. `WRKSLOTS_SLOT_TYPE` and `WRKSLOTS_TASK` let a project-owned
command consult validation-run evidence without guessing from a path. Its exit status means:

- `0`: the registered mechanism verified the agent is dead;
- `1`: the agent is alive;
- `2`: the mechanism cannot determine the answer;
- anything else: the check failed.

Only `0` satisfies that reclaim condition. The exact recorded process generation must independently
be dead, and the full process-use scan must find no cwd, executable, root, descriptor, mapping,
cgroup, or mount use. If the liveness source is degraded or stale, return `2`; unknown ownership is
not a free slot.

A slot imported from an older state file with no recorded owner identity cannot satisfy the
owner-death condition, even after its heartbeat expires. `recover-unbound-owner` can record what was
inspected but does not turn unavailable process evidence into proof of death. Preserve such a slot
and name it in the migration remainder rather than inventing an owner.

## HANDOFF.md

An agent slot may contain an untracked `HANDOFF.md` beside its checkouts. Its absence says nothing
about liveness. If it exists, reclaim refuses until `wrkslots read-handoff SLOT --coordinator-pid
PID` prints its exact UTF-8 contents and appends the contents and digest to the slot history. A
changed handoff must be read again. The recorded bytes remain in the history after the slot is
removed.

## Git remotes and salvage

For each `--repo NAME=PATH`, `create` uses the configured `origin` by default. Supply
`--remote NAME=REMOTE` to choose another configured remote and `--remote-url NAME=URL` when the
caller must verify its exact fetch URL. Wrkslots records a SHA-256 identity and refuses if the URL
changes.

A repository path is resolved from the configured project root, not from the caller's current
directory, and must be relative. Use an ordinary path inside the project root, or path components of
the form `../NAME` for one direct sibling repository. This is a normalized-path rule rather than a
byte-for-byte spelling requirement, but every other raw `..` traversal, every absolute path, and
every path with a symlink component is refused. Wrkslots stores the normalized relative path,
including `../NAME` for a sibling. Worktree destinations remain confined to the configured managed
worktrees directory.

For a dirty or unpushed agent checkout, reclaim constructs a commit without changing the checkout's
ordinary index or branch. It includes tracked, untracked, and ignored files except configured cache
paths, pushes the commit to `refs/heads/salvage/<machine>/<slot>/...`, reads that exact ref back, and
records the result. If the checkout was already clean and published, the existing remote containment
is recorded instead. A failed or unverifiable push preserves the checkout.

Initialized Git submodules are checked separately against the corresponding source repository's
remote URL. Each nested repository gets its own salvage commit and remote readback, so an
uncommitted file inside a submodule cannot disappear behind an outer gitlink that did not move.

## Import older slots

For a live slot already at its final managed path, run `import-existing` first as a dry run, then
repeat with `--apply --verified-live`, its live `--owner-pid`, and the current
`--coordinator-pid`. The owner may be the invoking process. A coordinator repairing another live
owner's slot must name an owner process that descends from that coordinator and whose working
directory is inside the slot; liveness without that path evidence refuses.

```sh
wrkslots import-existing slot01 --help
wrkslots import-existing slot01 --slot-type agent --coordinator-authorized \
  --agent codex-1 --task task-123 --purpose "continue task-123" \
  --repo product=../product --apply --verified-live \
  --owner-pid "$OWNER_PID" --coordinator-pid "$COORDINATOR_PID"
```

For a slot whose owner has exited, use the older version 3 `worktree-state.json` as evidence instead
of reconstructing ownership from the directory. Run `import-existing SLOT --from-state-file
worktree-state.json --source-host-id ID` as a dry run, then add `--apply` and the current
`--coordinator-pid`. The source row supplies the agent, task, purpose, allocation time, checkout
paths, and exact owner process generation. Because the older owner sidecar omitted the stable host
identity, `ID` must be the current source host's `/etc/machine-id`; a mismatch refuses. The imported
row starts a fresh heartbeat time-to-live. Its prior `active`, `lease-quarantined`,
`owner-lease-revoked`, or `release-requested` status is retained as evidence, never treated as
permission to remove. If the older row omitted its task or purpose, the active record says that the
field was not recorded; it does not invent what the slot held, and the exact source row remains
attached. The slot row's current task is used when present; the owner sidecar is only the fallback
when that row field is absent, because the sidecar describes the owner process at binding time.
Likewise, its agent name is retained as provenance rather than treated as a current assignment:
several older rows may name the same agent, and those rows do not prevent that name from owning one
new live slot. The ordinary registry still refuses two live assignments for one agent. A
source-file import also refuses while its exact recorded owner process is still live; that owner
must use the ordinary live import path instead.

Name each source row's checkout repositories with `--repo NAME=PATH`; `NAME` is the prefix of its
`NAME_path` field. A source row with nested paths remains nested even when newly created slots use the
flat layout. An empty residual slot directory can be imported with no `--repo`: its exact source row
is retained, and removal later verifies that the directory contains no checkout before recording
that there was nothing to salvage. A present checkout without a matching `--repo` refuses and
prints the missing flags. A row with no owner sidecar also refuses before writing a journal or
active row, because its owner death cannot be established.

Every applied import writes the complete candidate row to the append-only operation history before
publishing it in ACTIVE. If the importer exits after that write, any later participant runs
`wrkslots recover --coordinator-pid PID`; recovery does not reread the older state file or depend on
the original coordinator. Other unregistered slot directories are retained while the selected slot
is verified. Source rows whose physical directories are already absent remain in the older
file as history rather than being fabricated as active storage.

## Recover ownerless validation paths

Do not fabricate an ACTIVE owner for an unregistered validation path. `import-existing` remains the
route for a demonstrably live owner or an exact owner generation from a retained row; `remove` remains the
route for a registered row. An unregistered validation checkout or `validate-cargo-*` directory
with no recoverable owner instead uses `recover` directly:

```sh
wrkslots recover --coordinator-authorized --coordinator-pid "$COORDINATOR_PID" \
  --ownerless-validate-checkout worktrees/validate/validate-fresh-example \
  --repository product --completed-record ignored/validate/runs/validate-example.json

wrkslots recover --coordinator-authorized --coordinator-pid "$COORDINATOR_PID" \
  --ownerless-validate-cargo-home worktrees/validate/validate-cargo-example \
  --recovery-note "the retained run handle has no Cargo-home field"
```

The first form binds cleanup to an exact terminal run record. A recordless path requires a non-empty
explanation, but prose is not authority: wrkslots records the exact slot type, path, filesystem
identity, repository and commit where applicable, and its own positive determinations that no
retained record names the path, no live process uses it, and no authored work would be lost. A
checkout must be a clean linked worktree with ordinary Git state, no HANDOFF.md, and a HEAD contained
by a freshly fetched remote ref. Cargo homes must have the exact configured parent and
`validate-cargo-*` name and cannot be Git worktree roots. Both forms retain positive cwd,
executable, root, descriptor, mapping, and mount checks. Any dirty, unpublished, in-use, or
ambiguous path is preserved. The cleanup first renames the exact inode to a random same-parent path,
rechecks every fact, then deletes only that fenced path. Any later participant may resume the
journal with plain `recover --coordinator-pid PID`.

This targeted route does not hide anything from ordinary `status`: every other unregistered
directory is returned as a typed inconsistency beside the full readable roster. Status is
diagnosis, not permission to mutate. During a larger deliberate migration, the global
`--allow-existing-unregistered-worktrees` flag may accompany the exact cleanup command; it retains
all other paths.

## Recover registered validation rows with absent storage

If external cleanup removed disposable validation directories without retiring their ACTIVE rows,
generate an explicit bounded input from a fresh `wrkslots audit --format json`. Each registered row
includes its canonical `record_sha256`; copy only the intended rows into this form:

```json
{"schema": 1, "rows": [{"machine": "host", "slot": "validate-fresh-example", "generation": 1, "record_sha256": "<64 lowercase hex digits>"}]}
```

Inspect the read-only plan first, then apply the exact same file:

```sh
wrkslots recover-absent-validate-rows --input /path/to/rows.json
wrkslots recover-absent-validate-rows --input /path/to/rows.json --apply \
  --coordinator-authorized --coordinator-pid "$COORDINATOR_PID"
```

The command refuses agent rows, changed generations or row digests, holds, any path or Git
registration that still exists, unreadable handle/process/systemd evidence, and any live process or
user service that may use the row. A retained run handle contributes its exact service unit and,
when recorded, process generation to the present-tense liveness proof; its run state is not treated as a validation
outcome. The configured agent-liveness probe is likewise not an ownership authority: an agent may
restart elsewhere, while a validation may outlive the agent that launched it. The resource proof is
the exact recorded owner generation, retained service identity when present, an all-process path and
mount census, all user services/scopes/jobs, and absent Git and filesystem state.

The whole batch is preflighted before writing. Recovery then appends one archive transition per row,
records that durable prefix, and appends one ACTIVE-removal transition per row. A crash may expose a
prefix, not a fictitious all-or-nothing update; the journal makes that prefix resumable and refreshes
the compatibility snapshots before completion. Every archive keeps the established schema-2 shape,
records physical storage as removed (its disposition at archive time), and states that the validation
outcome remains unknown. Rerunning the same command or plain `wrkslots recover --coordinator-pid
PID` resumes safely. Do not infer a validation result or run number from this registry repair.
Cross-user process evidence uses passwordless `sudo -n` with fixed root-owned `find` and `grep`
binaries and refuses if that read-only census is unavailable or incomplete. Use `--format json` when
another tool needs the typed per-row outcome.

## Recovery and compatibility views

Every mutation appends a numbered, hash-linked JSON event before refreshing the readable ACTIVE,
ARCHIVED, hold, or journal view. Readers derive state from the event history whenever it exists, so
a stale compatibility view cannot override later evidence. A complete event left at an atomic-write
temporary path can be promoted by `recover --discard-partial`; malformed or ambiguous files refuse.

Create and removal journals contain the exact paths, Git identities, completed steps, salvage
receipts, and remaining work. If the mutable journal view is missing, `recover` reconstructs the
pending operation from the append-only history. Recovery rechecks every destructive precondition;
it does not trust the earlier participant's conclusion.

## Test scenarios

The source package includes deterministic and stress tests for concurrent creation, owner and
time-to-live disagreement, unavailable liveness, dirty and unpublished salvage, ignored-file
capture, validate deletion without salvage, unread handoffs, process use, path fencing, interrupted
operations, hash-chain corruption, missing compatibility views, and later-participant recovery.

The default test run uses a PID namespace for tests that only need isolated process evidence and
runs four host-visible process tests separately. This changes test cost, not coverage or assertions.
