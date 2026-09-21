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
2. The coordinator assigns the slot. The coordinator or assigned agent runs
   `create --slot-type agent --coordinator-authorized` for authored work, or
   `create --slot-type validate --coordinator-authorized` for disposable validation. An assigned
   agent supplies its own `--owner-pid`. The flag is a reminder and recorded provenance, not a
   permission boundary between same-user processes.
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
6. Before removing an agent slot, `remove` publishes unpushed commits and tracked and ordinary
   untracked files outside configured regenerable cache paths to the recorded remote. It never
   uploads gitignored content. It records and rechecks the exact remote ref and commit before
   deletion. A validate slot skips this step.
7. `recover` lets any later participant complete an interrupted create or removal from the durable
   history. It is not tied to the participant that began the operation.

Run `wrkslots quickstart` for copyable commands and `wrkslots COMMAND --help` for exact effects and
inputs.

## Coordinator assignment and creation

`create`, `register`, and `import-existing --apply` require both `--slot-type` and
`--coordinator-authorized`. Omitting either refuses before any worktree or lifecycle record changes
and tells the caller to ask the coordinator. This prevents accidental self-allocation; it does not
pretend that same-user processes have different operating-system permissions.

An assigned agent can run `create` with `--owner-pid` naming itself or an ancestor of the command.
Its live `--coordinator-pid` may run separately, including in another terminal pane; process
ancestry is not evidence of that assignment. The explicit flag records the assignment, while
the owner ancestry check prevents the agent from binding an unrelated process as owner.

Choose an owner process whose lifetime represents that one assigned agent. A shared multiplexing
supervisor, coordinator, or application server may be an ancestor of every worker command, but it
cannot identify which logical agent is alive; binding several agent names to that PID leaves every
row live until the shared process exits and defeats per-agent liveness and reclaim. Separate
coordinator and owner processes must be visible in the same Linux PID namespace so their `/proc`
identities can be read and rechecked.

The coordinator can instead invoke `create` to bind another live child owner. That path requires
the coordinator to be in the command's ancestry and the owner to descend from the coordinator.
Omitting `--owner-pid` also requires an invoking coordinator; the owner then immediately runs
`adopt` itself. Both exact process identities are checked after acquiring the mutation lock and
again before publishing the row. If either identity changes during provisioning, creation
refuses publication and retains the recovery journal.

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
while printing every unrelated inconsistency it retained. `read-handoff` validates the complete
authoritative history and the requested slot's record, directory, and Git worktrees, but unrelated
storage drift cannot withhold that slot's handoff. When importing a snapshot with no event log, it
first records the structurally validated snapshot without resolving unrelated repositories, after
separately validating the requested slot's repository and worktree. It records no change to unrelated
rows. Other
lifecycle mutations refuse by default if either managed directory contains a worktree without an
active wrkslots record. During a deliberate migration,
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

## Handoffs and retirement

Write a new handoff outside the checkout while its exact owner is still live:

```sh
wrkslots write-handoff SLOT --agent AGENT --owner-pid PID \
  --expected-generation N --from-file /path/to/HANDOFF.md
```

The input must be a regular UTF-8 file outside every Git working tree. This prevents a tracked,
shared repository document from being mistaken for slot-specific retirement intent. Direct-child
`HANDOFF.md` discovery likewise refuses a Git-tracked file; an untracked direct-child
file remains readable for migration.

The bounded UTF-8 contents go to a generation-bound control-plane sidecar, so writing the handoff
does not make a clean checkout dirty. The sidecar records the source classification, absolute path,
and exact file identity alongside its bytes. Before exposing a publication temporary file,
`write-handoff` records an owner-authenticated intent containing that complete deterministic
envelope; only exact canonical verification records completion. Reads accept the sidecar only when
both events match it. The sidecar is immutable once published; `finish` still refuses every tracked,
untracked, or ignored checkout change. In particular, it does not exempt a pre-sidecar `HANDOFF.md`
inside a flat-layout checkout.

`wrkslots read-handoff SLOT --coordinator-pid PID` prints the exact sidecar contents and durably
enqueues that slot generation for later retirement. Reading never removes the sidecar, an existing
checkout file, or the slot. If no sidecar exists, the command copies a direct-child `HANDOFF.md` into
the sidecar without moving or unlinking the old file. If both exist, their bytes must agree.
The command does not hold the state lock while it writes up to 1 MiB to the caller: it snapshots the
exact generation and file identity, writes and flushes the bytes, then reacquires the lock and
revalidates both before recording the read. A failed flush or concurrent change records no new read.
When the checkout file genuinely changed after an earlier read, ordinary read refuses and keeps
both copies. Published sidecars are immutable: the deprecated adoption option also refuses, so no
command silently replaces either artifact. An operator must preserve and reconcile the disagreement
outside this destructive workflow before a fresh slot can provide new retirement evidence.

Inspect the queue with `wrkslots retirement-queue --format json`. A coordinator may attempt a
bounded group with `wrkslots retire-pending --limit N --coordinator-pid PID --format json`. Each
item runs the ordinary removal state machine independently; a live owner, fresh heartbeat, hold,
process use, changed handoff, dirty or unpublished work that cannot be salvaged, remote mismatch,
or path-fence race retains the slot in the queue. Successful archived removal clears its sidecar.
Attempt events rotate blocked entries behind never-attempted and less-recently-attempted entries, so
one retained slot cannot starve the rest of a bounded queue. Lock contention is deferred; corrupt,
partial, or indeterminate state stops the batch and requires recovery instead of being mislabeled as
retained. Sidecar cleanup atomically renames the exact inode into content-addressed retired control
storage and never unlinks that retired pathname, so a same-UID replacement cannot be mistaken for
the acknowledged artifact. A direct-child checkout handoff is likewise moved by no-replace into an
identity-bound retired path before its fenced slot is removed; pathname recreation preserves both
copies and refuses. The exact handoff bytes remain in append-only history.

## Git remotes and salvage

For each `--repo NAME=PATH`, `create` uses the configured `origin` by default. Supply
`--remote NAME=REMOTE` to choose another configured remote and `--remote-url NAME=URL` when the
caller must verify its exact fetch URL. Wrkslots records a SHA-256 identity and refuses if the URL
changes.

Wrkslots also requires Git's single effective push URL to be byte-for-byte identical to that fetch
URL. Any non-empty `remote.<name>.pushurl`, multiple URL, or push-only rewrite such as
`url.<base>.pushInsteadOf` that changes the effective destination is refused; SSH and HTTPS
spellings are not normalized into equivalence. Before agent salvage performs its first fetch, ref
readback, or push, this authority check covers every parent checkout and initialized nested
repository in the salvage set. The exact URL and its digest are captured with each fully checked
salvage candidate and are then the only destination used by isolated network Git commands. A later
checkout-config change can only refuse the operation; it cannot choose a new network destination.
The isolated transport uses plumbing commands that do not apply URL rewrite configuration and
binds the fresh repository's config bytes before execution; changing that config also refuses before
the network command starts.

When `with-proxy` is installed on `PATH`, Wrkslots uses it for Git fetches, pushes, remote ref
readback, and post-provision hooks. The hook's children, including recursive submodule commands,
inherit the wrapper's environment. A wrapper failure stops the operation; Wrkslots does not retry
without it. On hosts without `with-proxy`, commands use the caller's network environment. Local
Git inspection uses no wrapper, and Wrkslots does not change global Git or proxy configuration.
Network Git commands keep global and system Git configuration disabled, but explicitly use the
GitHub CLI credential helper when an executable `FLEET_REAL_GH` or `gh` is available. This permits
authenticated HTTPS salvage without admitting unrelated global Git configuration into a removal.

A repository path is resolved from the configured project root, not from the caller's current
directory, and must be relative. Use an ordinary path inside the project root, or path components of
the form `../NAME` for one direct sibling repository. This is a normalized-path rule rather than a
byte-for-byte spelling requirement, but every other raw `..` traversal, every absolute path, and
every path with a symlink component is refused. Wrkslots stores the normalized relative path,
including `../NAME` for a sibling. Worktree destinations remain confined to the configured managed
worktrees directory.

For a dirty or unpushed agent checkout, reclaim constructs a commit without changing the checkout's
ordinary index or branch. It includes tracked and ordinary untracked files except configured cache
paths, pushes the commit to `refs/heads/salvage/<machine>/<slot>/...`, reads that exact ref back, and
records the result. If the checkout was already clean and published, the existing remote containment
is recorded instead. A failed or unverifiable push preserves the checkout.

Gitignored content is excluded by repository policy, not by size. It is never added with `--force`,
never included in the salvage status digest, and never uploaded to the recorded remote. Authored work
that must survive belongs in tracked or ordinary untracked paths, or in a separately recorded
artifact store; an ignored build tree is not remote preservation evidence.

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
route for a demonstrably live owner or an exact owner generation from a retained row; `remove`
remains the route for a registered row. Destructive recovery of an ordinary ownerless linked Git
worktree is disabled because wrkslots cannot atomically exclude both the checkout and its
same-UID-writable Git administration. Preserve and inspect it. The ownerless recovery route remains
available for `validate-cargo-*` directories:

```sh
wrkslots recover --coordinator-authorized --coordinator-pid "$COORDINATOR_PID" \
  --ownerless-validate-cargo-home worktrees/validate/validate-cargo-example \
  --recovery-note "the retained run handle has no Cargo-home field"
```

The Cargo-home form may instead use
`--completed-record ignored/validate/runs/validate-example.json` to bind cleanup to an exact
terminal run record. A recordless path requires a non-empty explanation, but prose is not
authority: wrkslots records the exact path and filesystem identity and independently verifies that
no retained record names it and no process uses it. Cargo homes must have the exact configured
parent and `validate-cargo-*` name and cannot be Git worktree roots. The cleanup retains positive
cwd, executable, root, descriptor, mapping, and mount checks, then deletes only the excluded path.
Any later participant may resume its journal with plain `recover --coordinator-pid PID`.

`--ownerless-validate-checkout` remains accepted so existing automation and pre-upgrade journals
fail with an explicit preservation message. It performs the bounded authored-work,
HANDOFF, submodule, remote-binding, and liveness checks, but it does not create a cleanup journal or
remove the linked worktree. An initialized submodule is itself a preservation reason even when Git
configuration would otherwise hide its state.

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

## Recover absent agent rows and ownerless agent worktrees

Agent storage is never treated as disposable. For an exact ACTIVE agent row whose directory is
already absent, take `machine`, `slot`, `generation`, and `record_sha256` from a fresh JSON audit and
inspect the read-only plan before applying it:

```sh
wrkslots recover-absent-agent-row SLOT --expected-generation N \
  --record-sha256 SHA256
wrkslots recover-absent-agent-row SLOT --expected-generation N \
  --record-sha256 SHA256 --apply --coordinator-authorized \
  --coordinator-pid "$COORDINATOR_PID"
```

The command requires the registered liveness authority to report dead, requires any recorded exact
owner generation to be dead, and performs the same full-host process, cgroup, mount, and user-systemd
census used for absent validation rows. It does not claim the vanished working tree was clean. Every
recorded checkout commit is pushed individually to a dedicated rescue ref and read back from the
remote before the exact stale Git worktree registration is removed. The archive is durable before
ACTIVE is changed and explicitly records that uncommitted, untracked, ignored, and HANDOFF contents
could not be inspected because storage was already absent.

An unregistered agent worktree is recovered without assigning it an owner, task, or handoff. Supply
the exact path and the Git identities established during inspection:

```sh
wrkslots recover-ownerless-agent-worktree worktrees/slots/example \
  --repository repo --head COMMIT --branch agent/example --remote origin \
  --remote-url-sha256 SHA256 [--handoff-sha256 SHA256]
```

Repeat with `--apply --coordinator-authorized --coordinator-pid "$COORDINATOR_PID"` only after the
plan agrees. The worktree must be one direct child of the managed agent root. A HANDOFF.md requires
the exact digest supplied after the coordinator reads it; the command rechecks it before every
destructive boundary and preserves it through the same remote salvage path. An unread or changed
handoff, any live process or user unit reference, a path or inode change, non-ordinary Git state, or
an ACTIVE/archive identity collision refuses. Tracked and ordinary untracked files outside
configured cache paths, and initialized nested repositories are committed to verified salvage refs
before the exact inode is fenced and removed. Any later participant can resume the journal with
ordinary `recover`.

The `worktrees/slots/ignored` path is not an agent worktree. Only when it contains the command's
one exact supported cache hierarchy can it be relocated intact outside the managed slot root:

```sh
wrkslots recover-ownerless-agent-cache
wrkslots recover-ownerless-agent-cache --apply --coordinator-authorized \
  --coordinator-pid "$COORDINATOR_PID"
```

This command accepts no arbitrary path, refuses symlinks, mounts, `.git`, `HANDOFF.md`, unexpected
top-level content, live use, or an occupied destination, and journals the same-inode relocation.

## Recovery and compatibility views

Every mutation appends a numbered, hash-linked JSON event before refreshing the readable ACTIVE,
ARCHIVED, hold, or journal view. Readers derive state from the event history whenever it exists, so
a stale compatibility view cannot override later evidence. A complete event left at an atomic-write
temporary path can be promoted by `recover --discard-partial`; handoff publication temps use
no-replace promotion and require independent durable authority. Owner-written temps must match an
outstanding full-envelope write intent, while checkout-file projections must match a prior
provenance-bearing nonqueued read. An orphan temp or a temp beside a conflicting durable sidecar is
preserved and refused. Malformed or ambiguous files refuse.

Create and removal journals contain the exact paths, Git identities, completed steps, salvage
receipts, and remaining work. If the mutable journal view is missing, `recover` reconstructs the
pending operation from the append-only history. Recovery rechecks every destructive precondition;
it does not trust the earlier participant's conclusion.

## Test scenarios

The source package includes deterministic and stress tests for concurrent creation, owner and
time-to-live disagreement, unavailable liveness, dirty and unpublished salvage, ignored-file
exclusion, validate deletion without salvage, unread handoffs, process use, path fencing, interrupted
operations, hash-chain corruption, missing compatibility views, and later-participant recovery.

The default test run uses a PID namespace for tests that only need isolated process evidence and
runs five host-visible process tests separately. This changes test cost, not coverage or assertions.
