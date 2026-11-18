# wrkslots cgroup liveness: hierarchy proof, bounded commands, dedicated lock

Status: design only, 2026-11-18. No product code is authorised by this
document. It governs #1829 wrkslots-cgroup-liveness-design and folds the
open failure set that motivates a rebuild rather than another patch:
the remove race (#1807), config-lock contention (#1276, #1808), the
status hang (#1827), /proc scans burning the remove budget (#1802,
#1772), reader skew from new persisted fields and the pinned-reader
problem (#1788), and the PF_EXITING /proc descriptor races. Prior art
is `ai_docs/plans/2026-11-16-slot-containment.md` (#1745); this design
keeps its `populated` proof and mount-privacy reasoning, moves the
container unit from one cgroup per slot to a hierarchy rooted at the
owner, and makes bounded cost a property of every command path, not
only of removal.

## Why a rebuild

Today liveness is assembled from four mechanisms that disagree on
cost and on meaning:

- Process identity. A recorded `ProcessIdentity` (`pid`,
  `start_ticks`, `boot_id`, `host_id`, `cgroup_path`) is judged by
  `_process_state`: open a pidfd, fall back to reading
  `/proc/<pid>/stat` and `/proc/<pid>/cgroup`, treat state `Z`/`X` as
  dead, and treat a reused PID as dead. The check is O(1) per owner
  but says nothing about children the owner spawned.
- Registered liveness. A configured per-agent command, or a batch
  command speaking `wrkslots-liveness-batch-request/v1`, is asked
  about each record. Read-only scans fan the per-agent form out on a
  thread pool. The answer is authoritative when it verifies, and
  unverifiable as a class when the command cannot run.
- Scoped census. When the recorded owner's cgroup is judged to still
  be evidence (`_owner_cgroup_is_evidence`), the members of that
  cgroup and its descendants (`_cgroup_member_pids`, bounded at 512
  directories) are checked one by one for cwd, root, exe and open
  descriptors naming the slot (`_scoped_cgroup_slot_users`).
- Host census. The fallback enumerates every process on the host,
  reads each one's links, descriptors, maps and mounts, and walks
  the slot with an external tool. Its cost is O(processes) plus
  O(slot files), it must special-case processes in `do_exit`
  (`PF_EXITING`, flag `0x00000004`) whose `/proc/<pid>/fd` entries
  vanish mid-walk, and it shares a deadline across a whole pass so
  that ten exiting compilers cost no more than one — a budget that
  loaded hosts still exhaust.

Each mechanism was added to close a hole in the previous one. The
result is correct on a quiet host and unbounded on a busy one:

- **Remove race (#1807).** A removal prepared against one registry
  revision can be overtaken while its census runs; the prepared
  proof then describes a state that no longer exists. The fix on
  `main` rebases prepared removals on the current log revision, but
  the census itself remains the long pole that makes the race
  window wide.
- **Config-lock contention (#1276, #1808).** The configuration lock
  is an exclusive `flock` on the project root directory itself,
  taken by `_locked_config` inside `_mutation_locks`. Sampling on a
  shared host found the lock free in only 35 of 315 samples while
  about 55 lanes heartbeated against the same project; every
  contender polls non-blocking (`LOCK_NB` in a 50 ms loop) and
  refuses as busy. #1276 moved fleet-wide validation out from under
  the lock for `adopt` and `create` (lock-free prepare, identity
  compare under the lock, bounded retries), which cut the exclusive
  window from tens of seconds to seconds, but most per-slot
  mutations still validate under the lock, and the lock object is
  still the project directory that unrelated tools also flock.
- **Status hang (#1827).** `status` phases its reads so process,
  disk and Git observation happen outside the lock, but the
  observation itself is unbounded: stats into dead FUSE mounts that
  never answer, subprocess waits with no deadline of their own, and
  a liveness thread pool whose `join` waits for the slowest probe.
  One wedged mount or one hung liveness command stalls the whole
  roster.
- **/proc scans burning the remove budget (#1802, #1772).** The
  standalone remove scenario carries a 60 s wall budget; under load
  the host census alone has taken 113 s (73 s solo). The verdict
  then depends on host load, which owner rule 346 forbids: no
  wall-clock budget may decide a gating verdict. #1772 bounded the
  ignored-file evidence to a summary plus manifest, but the census
  that decides “is anything using this checkout” is still the
  unbounded part.
- **Reader skew (#1788).** Every new persisted field is a
  compatibility event. Records written by a newer writer are refused
  by an older reader that does not know the field; a consumer that
  pins an older reader therefore pins the writer too. #1788 named
  the staged answer — `reader_floor` (the oldest reader that may
  read a record) and `writer_ceiling` (the newest field set a writer
  may emit) — but no record carries them yet, so each addition is
  still a flag day.
- **PF_EXITING descriptor races.** A process in `do_exit` has set
  `PF_EXITING` before it releases its mappings, descriptors,
  working directory and root. A census that opens
  `/proc/<pid>/fd/<n>` in that window races the teardown: the entry
  vanishes between the directory listing and the `readlink`, or
  reads back a target the process has already dropped. The code
  carries a shared exit-wait, zombie-state parsing and per-errno
  classification (`ENOENT`/`ESRCH` mean gone, everything else
  refuses) solely to survive reading a dying process's descriptor
  table. Any design that still walks foreign descriptor tables
  keeps this race forever.

The owner's direction, verbatim in intent: liveness from a hierarchy
of nested cgroups — a slot belongs to its owner's cgroup, and an
empty cgroup means a dead owner — with no global process or
filesystem crawl on any command path, and scans surviving only as a
cached, non-blocking extra signal. This document designs that.

## Terms

- **Owner**: the agent lane (or validation runner) that owns a slot.
  One owner may hold several slots over its lifetime, sequentially.
- **Owner cgroup**: the cgroup v2 group that contains every process
  of one owner generation. It is created by the launcher (see §1),
  not by wrkslots.
- **Slot cgroup**: a child cgroup of the owner cgroup, one per slot
  generation, holding every process that works in that slot.
- **Hierarchy**: owner cgroup at the top, slot cgroups beneath it,
  and any tool-created descendants (build steps, scopes) beneath
  them. `cgroup.events` `populated` counts descendants, so the
  hierarchy is the proof: a parent is populated while any child
  holds a process.
- **Contained slot**: a slot whose record names a slot cgroup and
  whose legitimate users all entered through the entry helper.
  Only contained slots use the cgroup proof.
- **Proof source**: the named evidence a verdict cites, printed in
  status output: `cgroup`, `pidfd`, `registered`, `cached-scan`,
  or `legacy-scan`. No verdict is ever stronger than its source.
- **Entry**: placing a process into a slot cgroup before it runs,
  through the one helper every launcher uses. Changing directory
  into a slot is not entry.
- **Escape**: a member that leaves its slot cgroup for a sibling or
  parent group it was not placed in, for example by starting a new
  service-manager unit that does not name the slot slice.

## 1. Liveness model

### How an owner's cgroup is created and known

Agentctl already launches every lane inside a service-manager scope.
The scope's cgroup is the owner cgroup. Concretely, a lane runs as a
transient scope under the user's manager, in a slice reserved for
agent lanes, so its cgroup path has the shape:

```
/user.slice/user-<uid>.slice/user@<uid>.service/agent-lanes.slice/agentctl-<agent>-<generation>.scope
```

The exact slice name is configuration, not protocol; what matters
is that the path is (a) created before the lane's first process
runs, (b) stable for the lane's lifetime, and (c) discoverable from
any process in the lane by reading `/proc/self/cgroup` and taking
the unified (`0::`) entry. Agentctl records the path, the manager's
boot identity (`/proc/sys/kernel/random/boot_id`), and the lane
generation in its own registry at launch, exactly as it already
reads `cgroup.events` to decide whether a recorded harness tree
still has processes (in both editions, treating a malformed or
unreadable file as unknown, never as dead).

Wrkslots learns the owner cgroup at the two moments it already
learns the owner process:

- **Create.** The creating process is inside the owner cgroup (the
  lane runs `wrkslots create` from its own pane). Wrkslots reads
  `/proc/self/cgroup`, verifies the path is under the configured
  agent-lane slice, and records it. It also opens a pidfd on the
  owner process and records the existing `ProcessIdentity`.
- **Adopt / register.** The adopting process names the owner PID.
  Wrkslots reads `/proc/<pid>/cgroup` through the same helper that
  builds `ProcessIdentity` today (`_read_process_cgroup`), verifies
  the path the same way, and records it.

The record gains three fields, all omit-when-absent so older
readers keep working (§4): `owner_cgroup_path` (the verified path),
`owner_cgroup_id` (the cgroup directory's inode, `stat` on the
path — an identity that survives a path being recreated), and
`slot_cgroup_path` (below). The `ProcessIdentity.cgroup_path`
field recorded today is the owner's leaf at capture time; the new
fields name the hierarchy root and the slot child explicitly, so a
later reader never has to guess which ancestor was meant.

If the process is not inside a recognisable lane scope — a person
running wrkslots from a login shell, a lane on a host whose manager
places scopes elsewhere — the fields stay absent and the slot is
uncontained. Uncontained is a first-class, honestly labelled state,
not an error.

### How a slot attaches to it

At create (or at adopt of a contained slot), wrkslots creates one
slot cgroup as a child of the owner cgroup:

```
<owner-cgroup>/wrkslots/<slot>-g<generation>/
```

Creation follows the delegation rules of the host (§1, edge cases):

- Where the owner cgroup is delegated (`Delegate=yes` on the lane
  scope, or an equivalent delegated subtree), wrkslots makes the
  directory directly (`mkdir`) and writes no controllers. A leaf
  that only needs membership and `cgroup.events` needs no
  controller enablement, so the “no processes in a group that has
  controlled children” rule (cgroup v2's no-internal-process
  constraint) is not triggered at the owner level: the owner
  cgroup holds no processes directly once slots exist — the lane's
  shell is moved into the first slot cgroup at creation, or the
  owner cgroup is created with a `supervisor` child for the lane's
  own processes, following the drain-into-a-child pattern the DAG
  runner already uses for delegated trees.
- Where delegation is unavailable, wrkslots asks the manager to
  create a transient slice beneath the lane scope
  (`systemd-run --user --scope --slice=<lane>-wrkslots.slice` or
  the `StartTransientUnit` call with `Slice=` naming a child
  slice). The manager owns the directory; wrkslots only reads its
  files. This is the same placement the slot sandbox already uses
  for per-slot limit slices; the change promotes that slice from a
  limits detail created at first boxed run to the slot's recorded
  container created with the slot.

Every later process that works in the slot enters the slot cgroup
through the entry helper, which is the existing slot run/box seam
extended to place first and exec second:

- A fresh command is placed with
  `systemd-run --user --scope --collect --slice=<slot-slice>`
  (or the direct `cgroup.procs` write where delegation allows).
  `--collect` is required so finished scopes are garbage-collected
  instead of lingering as empty units.
- A process that must keep its PID (a pane shell the terminal host
  is already watching) is moved without a fork by the manager's
  transient-unit call, as the box already does, falling back to
  `systemd-run --user --scope` when that call is unavailable.
- Entry is verified, never assumed: immediately after placement the
  entered process (or its launcher) reads its own
  `/proc/self/cgroup` and compares it with the recorded slot
  cgroup chain. A mismatch refuses the start and names both paths.
  The environment handed to members carries the slot cgroup path
  (`WRKSLOTS_SLOT_CGROUP`, beside the existing
  `WRKSLOTS_OWNER_CGROUP`) so nested tooling — the gate wrapper,
  the DAG runner, a second wrkslots invocation — nests beneath the
  slot instead of escaping sideways into a sibling scope.

Children inherit the cgroup at `fork`. Double-fork, new sessions
and closed terminals do not change membership; only an explicit
move or a new manager unit does. A build daemon, file watcher or
indexer for the slot is placed in the slot cgroup by the same
helper and therefore counts toward `populated` until it exits —
which is the desired semantics: a forgotten daemon blocks release
by name instead of being discovered by a host scan.

### How “dead” is decided

Liveness of a contained slot is two file reads:

1. `<slot-cgroup>/cgroup.events` → `populated`
2. `<owner-cgroup>/cgroup.events` → `populated`

plus the existence of each directory. The verdict table is
exhaustive; every other outcome is `unknown`, never `dead`:

| Slot dir | Slot `populated` | Owner `populated` | Verdict |
|---|---|---|---|
| exists | `0` | any | slot empty |
| exists | `1` | any | slot live |
| missing | — | `0` | dead (owner reaped, slot reaped with it) |
| missing | — | `1` | unknown (slot dir vanished under a live owner; fall back) |
| missing | — | missing | dead only if the removal journal recorded the empty proof and the unit stop; otherwise unknown |
| any | malformed/unreadable | any | unknown |
| any | any | malformed/unreadable | unknown for owner-level claims; slot verdict stands |

Owner liveness is the same table read at the owner level: the
owner is dead when its cgroup exists with `populated 0`, or is
missing under the journal condition above. “Slot belongs to its
owner's cgroup; empty cgroup = dead owner” is therefore literal:
when the last process of the last slot of a lane exits, the owner
cgroup reads `populated 0`, and every slot of that owner is
releasable without asking any other authority.

The recorded owner's pidfd check stays as a second, independent
proof for the owner process itself, and both must agree before a
release proceeds: `populated 0` proves no contained process
remains; the pidfd proves the recorded generation itself exited
(and was not merely moved). Read both twice — once when the
release opens, once immediately before deletion — so a member that
arrives between the reads flips `populated` back to `1` and is
seen. No step lists `/proc`, walks the slot, enumerates cgroup
members, or opens a foreign descriptor table. Member listing
(`cgroup.procs` of the slot cgroup only, bounded by the slot's own
size) appears only in a refusal, to name who blocks release.

Absence alone never proves death. A cgroup directory that
disappears without the journal entry — a manager restart, a manual
cleanup, a reboot — is `unknown` and falls back (§1, edge cases).
That rule is unchanged from the team-namespace design and from
agentctl's harness-tree handling.

### Edge cases

**Owner restart.** A lane that restarts gets a new scope and
therefore a new owner cgroup path and a new generation. The old
owner cgroup drains to `populated 0` and is collected by the
manager. Slots recorded against the old path judge `unknown` (the
directory is gone without a journal entry) until the restarted lane
re-adopts them, at which point adopt records the new owner path,
bumps the slot generation if the checkout was also recreated, and
the slot is contained again. A slot whose owner restarts and never
returns is reclaimed by the existing ownerless path: the pidfd
proof shows the recorded generation dead, the handoff and hold
guards still apply, and the census fallback (§2) — not the cgroup
— authorises the reclaim. Restart never makes a live slot read
dead, because the new cgroup is a different directory with a
different inode (`owner_cgroup_id` mismatches, so the old record
cannot be confused with the new one).

**Takeover.** A handoff transfers a slot to a different owner. The
receiving lane enters the slot through the entry helper, wrkslots
verifies the entrant's cgroup chain, and only then does the
transfer commit record the new `owner_cgroup_path`,
`owner_cgroup_id` and `slot_cgroup_path` (the slot cgroup is
re-created beneath the new owner; the old slot cgroup must read
`populated 0` before the move, or the transfer refuses as busy
naming the members). Until the commit, the slot still belongs to
the old hierarchy and judges by it. A crash mid-transfer leaves the
journal the recovery path already handles: the journal names both
hierarchies, recovery thaws any freeze, verifies which side
committed by the registry revision, and never judges from the
half-moved state.

**Hosts or agents without cgroup v2 delegation.** Three honest
degradations, each labelled in status by its proof source:

1. *No delegation, manager placement available.* The slot slice is
   manager-created (§1). The proof is identical; only creation
   goes through the manager. This is the expected shape on most
   hosts, because the lane scope itself is manager-created.
2. *No cgroup v2 at all, or a subtree mount that hides the
   hierarchy.* At adopt/create the whole-hierarchy check (the
   mount at the cgroup root must be cgroup v2 showing the full
   tree, with no covering mount beneath it — the check the retained-
   unit census already performs) fails, the cgroup fields stay
   absent, and the slot is uncontained. Liveness comes from the
   pidfd plus the registered liveness authority, exactly as today,
   and status says `pidfd`/`registered`, never `cgroup`. There is
   no scan on the command path even here (§2): the host census is
   a cached background signal, and an uncontained slot whose cache
   entry is stale judges `unknown` and refuses destructive action
   rather than scanning synchronously.
3. *Agent without a lane scope* (a person at a shell, a worker on
   its own node with no local manager). Same as (2): uncontained,
   labelled, and judged by pidfd/registered liveness. A worker on
   its own node is recorded as not-a-member (remote); its liveness
   comes from its session state, and no host proof is derived from
   its absence. A node that runs slots locally applies this same
   design locally.

In every degraded case the failure is fail-closed and visible:
`doctor` reports, per host, how many slots are contained, how many
are uncontained and why (no delegation, no v2 hierarchy, no lane
scope), so the fallback population is a number an operator can
drive to zero rather than a silent majority.

**Frozen and stuck members.** Release may freeze the slot cgroup
(`cgroup.freeze`) after closing entry, so a member that migrates
in despite the close arrives frozen and flips `populated` back to
`1` where the second read sees it. A member in uninterruptible
sleep (D-state) keeps `populated 1` and a kill pending until it
wakes; removal waits on the condition `populated 0`, names the
stuck members from the slot's own member list, and never judges
them dead. A crash after freezing leaves members frozen; journal
recovery thaws before anything else.

**Escapes.** A same-user process can write itself into another
cgroup or start a sibling manager unit that does not name the slot
slice. The hierarchy is an accident boundary for cooperating
tooling, as the sandbox already documents itself, not a security
wall. Defences are: slot-aware wrappers always name the slice, the
environment carries the slot cgroup so nested tooling nests instead
of escaping, entry verification catches never-entered lanes, and
the cached background scan (§2) sweeps for generation-marked
escapees as an audit signal — it can raise a warning and mark a
slot's cached proof stale, but it never runs on a command path and
never by itself authorises or blocks a release. Validation slots,
whose checkouts are disposable, trust `populated` alone; whether
agent slots also require a clean escape sweep from the cache
before a census-free release is open question 1 (§5).

## 2. Command paths with bounded cost

The invariant: **no command path enumerates host processes or
walks a filesystem it does not own.** Every command's cost is
O(1) (a fixed number of small reads) or O(slots) (one small read
per slot in the roster it prints), never O(processes) and never
O(files in a checkout). The table states what each command reads
after this design; anything not listed is not read.

| Command | Reads | Cost |
|---|---|---|
| `status` (one slot) | the slot record, its slot + owner `cgroup.events`, owner pidfd, hold/handoff sidecars | O(1) |
| `status` (roster) | the roster (already O(slots) state), then one `cgroup.events` pair + pidfd per visible record | O(slots) |
| `heartbeat` | the slot record, the dedicated lock (§3), one event append | O(1) |
| `hold` / `unhold` / `release` | as heartbeat | O(1) |
| `create` | config, the caller's own `/proc/self/cgroup`, one `mkdir`/manager call, the slot record write | O(1) plus the Git worktree creation it already does |
| `adopt` | config, `/proc/<owner-pid>/cgroup` for the named owner only, the slot record write | O(1) |
| `remove` (contained) | slot record, slot + owner `cgroup.events` (twice), owner pidfd (twice), journal write, checkout deletion | O(1) proof; deletion is O(slot files) as today and runs outside the lock |
| `remove` (uncontained) | slot record, owner pidfd, registered liveness (one bounded subprocess), cached-scan entry (§2) | O(1); refuses as `unknown` if the cache is stale |
| `recover` | the named journal, the journals' recorded hierarchies, registry revision compare | O(journals named), never a fleet scan |
| `doctor` | roster, per-slot proof-source tally, cache freshness, `populated` per contained slot | O(slots) |
| `ttl-report` / retirement queue | roster + `cgroup.events` per candidate | O(slots) |

Three consequences deserve emphasis.

**Status never stats a checkout.** Today status observes disks and
Git per record, which is where dead FUSE mounts enter: a `stat`
into a wedged Eden mount never returns, and the phased read's
deadline can only abandon the whole roster, not the one bad row.
After this design, liveness for status is the cgroup pair above.
Storage consistency, Eden capacity and Git observations become
separate, explicitly requested sections (`status --storage`,
`doctor`) that probe each checkout in an abandoned child process
with its own deadline — the pattern the dead-mount probe already
uses (probe in a child that can be left behind, never a thread
that must be joined) — so one wedged mount degrades one row to
`unknown` and cannot stall the roster. Subprocess waits all carry
deadlines; the liveness thread pool disappears because there is
nothing left to fan out: per-slot liveness is a file read, and the
registered batch command, where configured, is one bounded
subprocess for the whole roster with a hard timeout and a byte
limit, whose failure marks every subject `unverifiable` together
(as today) instead of joining per-agent probes.

**Removal never scans.** A contained removal's proof is the table
in §1. The 60 s remove budget stops being a verdict: removal waits
on the condition `populated 0` (with a generous backstop that
reports no result, exit 75, instead of failing — the shape the
standalone scenario already adopted), names the members that block
it from the slot's own `cgroup.procs`, and otherwise proceeds.
There is no host census to burn the budget, so the #1802 class —
verdict decided by whether a scan finished before a wall clock —
cannot recur for contained slots. Uncontained removals consult the
cache (below) and the registered authority; if neither can answer
freshly, they refuse as `unknown`. They never fall back to a
synchronous host scan. The operator escape hatch remains an
explicit `wrkslots audit` / `doctor --scan` that runs the census
on demand, off any command path, and says so in its output.

**Where scans survive: a cached, non-blocking extra signal.** One
per-host background writer — the sweeper (§5) or an opportunistic
refresh piggy-backed on `doctor --scan` — runs the legacy census
at most once per freshness window (default 120 s, configuration),
writes its result to a cache file in the control area
(`scan-cache/<host>.json`, one entry per slot: verdict, proof
detail, census timestamp, census duration), fsyncs and atomically
replaces it. Command paths read the cache only when they need an
uncontained verdict; they never wait for a refresh, never start
one, and treat an entry older than the window as absent
(`unknown`). The cache is evidence about the past, labelled
`cached-scan` with its age in status output, and it can only ever
make an uncontained slot *more* conservative: a cached “in use”
blocks a removal, a cached “unused” authorises one only together
with a dead pidfd and a dead registered owner. It never overrides
a cgroup verdict in either direction. PF_EXITING handling, the
exit-wait and the per-errno census classification all move into
this background writer, which is the one place that still walks
foreign descriptor tables — off the hot path, where a race costs a
stale cache entry instead of a stalled command.

Cost is testable without wall clocks: tests count files opened and
members read (the hermetic fake-cgroup pattern of
#1482 census-gone-tests-hermetic), and assert that thousands of
unrelated fake processes change neither the verdict nor the count.
No test asserts elapsed time (owner rule 346).

## 3. Locking

The configuration lock today is an exclusive `flock` on the project
root directory. That choice has three costs: the lock object is a
directory other tools also open and flock, so contention is
indistinguishable from interference; every contender polls
non-blocking and refuses, so under heartbeat load (55 lanes, lock
free in 35 of 315 samples) ordinary mutations starve; and the lock
is held across whatever the command still does beneath
`_mutation_locks`, which for most per-slot mutations still includes
fleet validation.

The rebuild separates the three jobs the one lock currently does:

1. **Dedicated lock object.** A single file,
   `<control>/config.lock`, created by `init` with mode `0600`,
   never a directory, never the project root. All configuration and
   registry mutations flock this file and nothing else. A consumer
   that needs to exclude wrkslots mutations flocks the same file
   (the held-lock delegation protocol — pidfd-proved descriptor
   adoption via `WRKSLOTS_HELD_CONFIG_LOCK_FD` — is carried over
   unchanged, re-pointed at the new path). The project directory is
   never flocked again, which ends the interference class where a
   run-population lock and a wrkslots mutation block each other by
   sharing a directory inode.
2. **Blocking waits with deadlines.** Contenders wait on the lock
   (`flock` without `LOCK_NB`, bounded by `--wait-lock`) instead of
   poll-refusing. The kernel queues waiters; a heartbeat that
   arrives during a 200 ms mutation waits 200 ms instead of
   refusing and lapsing a TTL. The deadline is a backstop that
   reports busy (exit 75, holder named from `/proc/locks` read
   *after* the deadline expires — a diagnostic read, not a
   contention mechanism), never a verdict about the work itself.
   Waiters are fair in arrival order to the extent the kernel's
   flock queue is; no userspace turnstile is needed once holds are
   short, and the writer-intent turnstile on the registry lock is
   retired with the contention that required it.
3. **Short holds.** Under the dedicated lock a mutation does only:
   re-read the configuration and refuse if it changed (the existing
   coherence check), compare the registry identity (one `lstat` per
   registry file, the #1276 identity), write or replace the one
   record it touches, append its event, and fsync. Everything else
   — fleet validation, storage checks, Git probes, salvage
   preparation, checkout deletion — happens before the lock
   (lock-free prepare, as #1276 built for `adopt`/`create` and
   #1194/#1287 generalised) or after it (under the per-slot fence
   and journal, as #1735 did for salvage). The target hold is one
   record write: single-digit milliseconds on a loaded host, two
   orders of magnitude below the multi-second windows measured for
   #1276. `_run_prevalidated_mutation` becomes the only mutation
   shape; the remaining per-slot commands (`heartbeat`, `hold`,
   `release`, `transfer`, `finish`, and the rest named in #1276's
   notes) convert to it one at a time (§5), and each conversion is
   tested by counting what is read under the lock, not by timing
   it.

Per-slot state keeps its own locks exactly as today (the `ACTIVE`
registry lock and the per-state-file lock in `_locked`), with the
same blocking-with-deadline discipline. Lock ordering is fixed and
documented in one place: dedicated config lock → registry lock →
state-file lock → per-slot fence. No path takes them in another
order, and no path holds any of them across a subprocess, a network
call, a cgroup wait, or a filesystem walk.

This folds #1276 and #1808 rather than superseding them: #1276's
lock-free validation and identity compare are the prepare/commit
split above, extended from two commands to all of them; #1808's
contention measurement is the acceptance test — the same sampling
method must show the dedicated lock held only in short bursts and
ordinary heartbeats never refused while another slot's removal
salvages.

## 4. Data and compatibility

### What persisted state changes

All changes are additive and omit-when-absent; no existing field
changes meaning, and no field is removed.

- `ActiveRecord` gains, per slot: `owner_cgroup_path`,
  `owner_cgroup_id` (inode), `slot_cgroup_path`, and
  `liveness_source` (the proof source of the last verdict, for
  audit). The existing `owner: ProcessIdentity` (with its
  `cgroup_path` leaf) is unchanged.
- The slot record's JSON gains the same keys. A record without them
  is a legacy/uncontained record and reads exactly as today.
- The control area gains: `config.lock` (the dedicated lock file),
  `scan-cache/` (a collection directory of its own, one file per
  host, per the layout rule that a growing collection gets its own
  directory), and `wrkslots/` beneath each owner cgroup is *not*
  control-area state — it is kernel state, recreated at adopt.
- Configuration gains one optional key, `cgroup_liveness`
  (`auto` | `require` | `off`, default `auto`): `auto` contains
  what it can and labels the rest, `require` refuses to create an
  uncontained slot, `off` disables the cgroup proof entirely (the
  rollback switch, §6). Older readers are told to list it in
  `ignorable_keys`, as with prior optional keys.

No event-log schema changes. New events (`slot-contained`,
`slot-recontained`) are additive kinds older readers already skip
by kind allowlist; if a deployment's reader refuses unknown kinds,
the events are emitted only after the writer ceiling (§4) allows.

### How to migrate without breaking pinned readers

The pinned-reader problem (#1788) is that a consumer may run a
reader older than the writer that produced a record. The migration
therefore stages readers before writers, using the
`reader_floor`/`writer_ceiling` discipline #1788 named:

1. **Readers first (floor).** A release ships readers — Python,
   Rust, and the registered-liveness/status consumers — that
   *accept* the new fields and ignore them for verdicts, and that
   report `reader_floor` (the oldest record shape they accept).
   Records do not change yet. A mixed fleet is safe because nothing
   new is written.
2. **Writers second (ceiling).** Only when every deployed reader
   reports a floor that accepts the new fields does the writer
   raise its `writer_ceiling` and begin emitting them, starting
   with records for *new* slots only. Existing records are not
   rewritten in place; a slot gains cgroup fields at its next
   adopt, transfer or generation bump. There is no flag day and no
   registry rewrite.
3. **Verdicts last.** Only when the writer ceiling has been raised
   fleet-wide does any command *act* on the cgroup proof (as
   opposed to recording and displaying it). Until then status shows
   the would-be verdict beside the authoritative one
   (`proof_source: pidfd (cgroup would say: dead)`), which is the
   evidence base for trusting the flip.

Each stage is reversible by configuration: lowering the writer
ceiling stops new fields being written (readers keep accepting
them); `cgroup_liveness: off` stops verdicts being taken from them.
Downgrading a writer never makes an existing record unreadable,
because fields are only ever added omit-when-absent.

The paired-move plan for consumers is mechanical and is part of
the change, not a follow-up: for each consumer that pins a reader,
the landing unit is a *pair* — the reader upgrade and the writer
upgrade as two commits that must land reader-first, with the
consumer's pinned version bumped in the same change that raises
the ceiling. The validation for the pair is the schema-skew suite
(the newer-writer/older-reader fixtures of #1788) run in both
directions: new writer records read by the oldest supported
reader, and old records read by the new reader, must both succeed
with identical verdicts on the fields the old reader knows. A
consumer that cannot move is accommodated by the ceiling: the
writer simply never raises it for that project's registry until
the consumer's reader moves, and `doctor` names the project and
the blocking floor. No consumer is ever broken by another
consumer's upgrade, because ceilings are per-registry
(configuration), not global.

## 5. Staged implementation plan

Each stage lands alone, in both editions where the surface exists
(the census-fallback commands are Python-only today; the Rust
edition refuses to them and the gap is recorded), with
deterministic tests that wait on conditions (a file's content, a
barrier, a recorded state) and count operations rather than
asserting elapsed time. ETAs are engineering time for one lane,
excluding gate queueing.

| Stage | Content | ETA | Closes / advances |
|---|---|---|---|
| 0. Record and display | New record fields (omit-when-absent), reader acceptance in both editions, `cgroup_liveness` config, status shows `proof_source` and the shadow cgroup verdict. No verdict changes. | 2 d | #1788 (reader_floor half) |
| 1. Dedicated lock | `config.lock`, blocking-with-deadline waits, held-lock delegation re-pointed, project directory never flocked. No hold shortening yet. | 2 d | #1808, half of #1276 |
| 2. Hierarchy creation | Agentctl records owner cgroup at launch; wrkslots records owner/slot cgroups at create/adopt, creates slot cgroups (delegated `mkdir`, manager fallback), entry helper with verified placement, `WRKSLOTS_SLOT_CGROUP` in member environment. Slots contained, verdicts still from pidfd/registered. | 4 d | — (enables 3) |
| 3. Cgroup liveness verdicts | `populated` table of §1 becomes authoritative for contained slots; pidfd as second proof; status roster reads only cgroup pairs; liveness thread pool and per-agent fan-out removed from status; storage/Git observation split into explicitly requested, child-probed sections. | 3 d | #1827, #1802 (contained half) |
| 4. Census-free removal + cached scan | Contained removal waits on `populated 0`; background scan-cache writer (the legacy census, PF_EXITING handling and all, moved off-path); uncontained removal consults cache + registered authority and refuses `unknown` rather than scanning. | 4 d | #1807, #1802, #1772 (census half) |
| 5. Lock shortening everywhere | Every per-slot mutation converted to prepare/commit (`_run_prevalidated_mutation` as the only shape); holds measured by counted reads under the lock; contention resampled with the #1276 method. | 4 d | #1276, #1808 (remainder) |
| 6. Daemon and Eden placement | Per-slot build/watcher daemons placed in the slot slice with stop-before-proof; Eden slots contained once the per-slot daemon placement lands; mount-privacy completion on top of the private-bind fix. | 5 d | Eden gap of §1; advances the Eden containment issues |
| 7. Writer ceiling + verdict flip fleet-wide | Per-registry ceilings raised as consumers' readers move (paired moves, §4); `require` mode available; legacy census retained only as the cache writer and the explicit audit command. | 2 d + consumer moves | #1788 (writer_ceiling half), #1772 (remainder) |

Total: about five engineering weeks for one lane, with stages 0–4
(the owner's failure set minus the Eden gap) landing in the first
three. The doc's own target — ready for review by midday ET on
2026-11-18 — is met by this document; implementation starts only
on the coordinator's go.

Decisions only the owner can take (defaults in brackets, applied
unless overruled):

1. Agent slots: `populated` alone, or plus a clean cached escape
   sweep before a census-free release? [Validation slots:
   `populated` alone. Agent slots: `populated` plus pidfd, with
   the cache as warning only, until one escape is observed.]
2. New agent slots default to closed access (ambient editor/shell
   use keeps the slot uncontained)? [Yes.]
3. Adoption at restart only, live migration of a running tree
   never? [Yes — a split tree is a worse proof than an honest
   legacy one.]
4. Freeze during the release proof, or close-entry without freeze?
   [Freeze, with thaw-first recovery.]
5. Eden containment waits for per-slot daemon placement; how
   urgent relative to stages 3–4? [After 4; Eden slots stay
   uncontained and labelled meanwhile.]
6. Cleanup bound for the sweeper (maximum cleanup-pending
   checkouts before new validations queue)? [Configuration, no
   wrkslots default; the sweeper is one named per-host service.]

## 6. Risks and alternatives considered

**Risks.**

- *Escape by explicit move.* A cooperating-but-careless tool that
  starts a sibling scope silently removes its processes from the
  proof. Mitigated by slice-naming wrappers, the environment
  chain and entry verification; residual risk is accepted and
  auditable through the cache, and is strictly smaller than
  today's post-scan entrant window, which every slot type already
  has.
- *Manager restart.* Transient scopes and placement evidence may
  vanish while processes survive elsewhere. Every such record
  judges `unknown` and falls back; absence never reads as death.
  The blast radius is a conservative refusal, not a wrong deletion.
- *Delegation variance.* Hosts differ in whether the lane scope is
  delegated and in how the manager names slices. The
  whole-hierarchy visibility check at adopt/create fails closed to
  uncontained, and `doctor` reports the reason, so variance shows
  up as a labelled fallback population rather than as flaky
  verdicts.
- *D-state members.* A wedged FUSE or NFS operation can pin
  `populated 1` indefinitely. Removal names the member and waits;
  this is honest but can strand a checkout. The alternative —
  judging a D-state process dead — is how data is lost, and is
  rejected.
- *Two editions diverging.* The Python and Rust editions must
  agree on the verdict table byte-for-byte. The differential
  fixtures (fake cgroup trees, populated 1/0/malformed/missing)
  are shared, and a verdict change lands in both or neither.
- *Rollback.* `cgroup_liveness: off` returns every slot to
  pidfd/registered verdicts; records keep their fields and stay
  readable. Rollback never requires a registry rewrite.

**Alternatives considered.**

- *Per-slot cgroup without the owner hierarchy* (the 2026-11-16
  containment design standing alone). Rejected as the *sole*
  model, kept as the slot level of this one: removal asks about
  one slot, but the owner's question — “is this lane gone, and are
  all its slots therefore reclaimable?” — is asked just as often
  (TTL teardown, lane restart, coordinator recovery), and only the
  hierarchy answers it in O(1). A fleet of unrelated slot cgroups
  answers it in O(slots) pidfd checks with a window between them.
- *Pidfd-only liveness* (drop the census, keep per-owner pidfds).
  Rejected: a pidfd proves the recorded generation, not its
  children. The orphaned-child case is exactly what the scoped
  census was built to catch, and it is the common case (build
  daemons, watchers), not an exotic one.
- *Keep the host census, make it faster* (parallelise, bound,
  cache in-process). Rejected: the cost stays O(processes), the
  PF_EXITING descriptor race stays, and the verdict stays
  load-dependent. Every optimisation of a crawl leaves a crawl.
- *Kernel notification instead of polling `cgroup.events`*
  (inotify/poll on the file, `cgroup.kill` for teardown).
  Adopted where it helps — removal waits by polling `populated`
  with backoff, which is a condition wait, and `cgroup.kill`
  remains the teardown for contained members that must be stopped
  — but notification does not change the verdict model, only its
  latency, so it is an implementation detail of stage 4 rather
  than a design alternative.
- *PID namespaces as the proof* (the parked per-team keeper
  design). Rejected for the reasons recorded when it was parked:
  from inside a team's PID namespace a lane cannot create the
  manager scopes gates need, and a setuid helper fails its owner
  check under the identity mapping. Cgroups need no join ceremony,
  no fresh `/proc`, no keeper and no identity mapping, and work
  identically for every harness.

## References

- #1276 adopt-lock-narrow — lock-free validation, registry identity.
- #1300 team-pid-namespaces — Phase 1 probes, Phase 2a pidfd and
  scoped census, parked keeper design.
- #1745 / `ai_docs/plans/2026-11-16-slot-containment.md` — the
  `populated` proof, mount privacy, outside-access classification.
- #1757 — liveness-first journal classification.
- #1769 — exiting-process census judged by condition, not window.
- #1772 — bounded ignored-file evidence.
- #1788 — reader_floor / writer_ceiling for persisted fields.
- #1807 — remove race on prepared removals.
- #1808, #1827, #1802 — contention, status hang, scan budget (this
  design's motivating measurements, §“Why a rebuild”).
