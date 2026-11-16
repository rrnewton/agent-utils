# Slot containment: release proof without host-wide scans (#1745 wrkslots-slot-containment-design)

Status: design proposal, 2026-11-16. No code is authorized here; nothing is shipped until its phase lands.

## Summary

Today a slot removal proves "nothing is using this checkout" by
scanning the whole host: every process, plus a directory walk of the
slot. On a loaded host that scan times out, so removals stall and
finished checkouts linger for hours. The scan is also the wrong
shape: its cost grows with every unrelated process on the machine.

This proposal gives each slot its own kernel container, a dedicated
per-slot cgroup, and runs every legitimate user of the slot inside
it. Liveness is then one small file read: the cgroup says whether it
still holds any process. Release is just as small: entry closed,
cgroup empty, recorded owner dead. No host scan runs. Where a slot
cannot be contained that way, today's scan stays as the fallback,
and status output says which proof answered.

The trade is stated plainly: the new proof is weaker than a
completed host scan. It proves facts about contained processes only.
Outside access is classified below as harmless, must-enter, or gap;
a gap closes later or keeps the fallback. Nothing silently narrows.

## Terms

Plain definitions for this document. Later sections use no other
special words without defining them where they appear.

- **Slot**: one registered worktree checkout managed by wrkslots,
  with a record, an owner process, and a lifetime that ends in
  removal.
- **Cgroup**: a kernel group for processes (control group, version
  2). Processes belong to exactly one cgroup. Children inherit the
  parent's group at fork. Limits and membership can be read from
  small files under the cgroup directory.
- **Slot cgroup**: the one cgroup dedicated to one slot generation.
  It is created when the slot is created or adopted, recorded in
  the slot record, and removed after the slot is released.
- **Contained slot**: a slot whose record names a slot cgroup and
  whose legitimate users all entered through the entry helper
  (section 2). Only contained slots may use the new proof.
- **Populated**: the kernel flag in the file `cgroup.events`.
  `populated 1` means the cgroup or any child cgroup still holds at
  least one process. `populated 0` means none do.
- **Pidfd**: a kernel handle pinning one process generation.
- **Namespace**: a kernel boundary that gives a process its own
  view of one thing: process IDs (PID namespace), mounts (mount
  namespace), or user IDs (user namespace). Namespaces are optional
  in this design; the cgroup carries the proof.
- **Keeper**: in the earlier per-team design, a minimal idle
  process that holds a team's PID namespace open as its PID 1. This
  proposal has no keeper.
- **Census**: today's host-wide proof. It lists every host process
  and checks each one's working directory, root, executable, open
  files, memory maps, and mounts against the slot path, plus a
  directory walk tool (`lsof`) over the slot.
- **Fence**: the existing removal step that renames and re-mounts a
  slot at a private fenced path, so no new user can arrive by the
  old name while removal finishes.
- **Entry**: placing a process into a slot cgroup before it runs,
  through the one helper every launcher uses. Changing directory
  into a slot is not entry.
- **Escape**: a process that was inside a slot cgroup and left it,
  for example by starting a new service-manager unit outside the
  slot. Double-fork and new sessions are not escapes; they stay in
  the cgroup.
- **Donor**: a helper authorized to work inside another lane's slot.
- **Receipt**: a recorded verdict about one exact commit (section 8).
- **Cleanup-pending**: receipt written, checkout not yet removed.
- **Sweeper**: the one named per-host cleanup service (section 8).
- **Proof source**: the named evidence printed in status output.

## What this changes relative to #1300 team-pid-namespaces, and why

The governing prior work is `ai_docs/plans/2026-11-12-team-pid-namespaces.md`
(#1300 team-pid-namespaces). Its Phase 1 design and probes stand.
Its Phase 2a landed: dual owner-identity fields, owner liveness
that tries a pidfd first, a census scoped to the owner's slot
cgroup, and hermetic census tests (#1482 census-gone-tests-hermetic).
Its Phase 2b, the per-team keeper and the namespace join, was
parked and never authorized. The parking note records why: from
inside a team's PID namespace, a lane cannot create the
service-manager scopes that gates and wrkslots need, because the
manager sees such a peer's process ID as 0; and a setuid helper
fails its owner check under the identity mapping.
Every gate-running lane would need those steps proxied back to the
host view.

This proposal supersedes that parked end state. It does not
restart from zero.

Kept from #1300 team-pid-namespaces:

- The Phase 1 probe results, including the two traps: a PID
  namespace without a freshly mounted `/proc` fools every reader,
  and a PID namespace affects children only, never the process that
  creates or enters it.
- All of Phase 2a, including evidence naming in status output and
  the rule that no slot is judged dead because newer evidence is
  absent.
- The host-view discipline: proofs of death are made from the
  initial namespace view, and every recorded process ID names the
  namespace it belongs to.
- Per-run PID namespaces for boxed validations, as an option where
  the launcher allows them (phase 6 below).

Changed, with reasons:

| Question | #1300 team-pid-namespaces | This proposal |
| --- | --- | --- |
| Container unit | One per team | One per slot |
| Load-bearing primitive | PID namespace held by a keeper | Slot cgroup `populated` flag |
| Who must join what | Every lane joins the team namespaces | Every slot user enters the slot cgroup |
| Removal proof | Check each team member for slot use | Empty cgroup after entry closes |
| Single point of failure | Keeper exit kills the team | None; empty is the normal end |
| Manager access from inside | Blocked; needs a host broker | Unchanged; no namespace is required |

Why the unit moves from team to slot: removal asks about one slot,
not about a team. A team container still needs a member-by-member
check to answer it. A slot container answers it directly.

Why the primitive moves from namespace to cgroup: cgroup
membership needs no join ceremony, no fresh `/proc`, no keeper, and
no user-identity mapping, so the launcher failures that parked
Phase 2b do not arise. Placement into a cgroup is also blind to
which harness runs (section 2), while a user-namespace box today
starts only one of the three harnesses. The cost of the move is
real: a cgroup is a cooperative boundary for processes running as
the same user, not a security wall. Section 6 treats escapes
honestly on that basis.

Why Phase 2a is not enough: its scoped census still enumerates
members and checks each one, the batch validation path still runs
a shared host census with a large time bound, and the pidfd step
labels the evidence while the decision still re-reads host state.
The step that actually removes enumeration is trusting
`populated` itself, under the entry rules below. That trust, and
its limits, is the subject of this document.

## 1. The model

### One slot cgroup per slot generation

When a contained slot is created, registered, or adopted at a
restart, wrkslots creates or reuses one durable slot cgroup and
records it in the slot record: the cgroup path, the unit name that
owns it, and the boot identity under which it was observed. The
natural home already exists: boxed runs today put limits on a
per-slot slice under an all-slots slice, and run each command in a
short-lived scope inside it (`py/wrkslots/sandbox.py`). This
proposal promotes that slice from a limits detail created at first
boxed run to the slot's recorded container created with the slot.
Every later scope for that slot nests inside it. Because
`populated` counts child cgroups too, nesting can only add
members the proof sees; it cannot hide them.

Namespaces layer on top, optionally:

- A mount namespace, built by the existing box (user-namespace or
  root-helper entry), gives the slot a private mount view
  (section 3). It is the write-sandbox. It is not the proof.
- A PID namespace, per validation run, gives that run a private
  process view. It is never required for liveness or release.

A slot with no namespaces at all can still be contained: cgroup
placement alone (today's `cgroup` isolation mode) carries the
proof. Keeping proof and sandbox separate is deliberate. The proof
must work for every harness on day one; the mount sandbox does not
(section 2).

### Liveness and release

Liveness for a contained slot is: read `cgroup.events` in the slot
cgroup. `populated 1` means live; where a run also has namespaces,
their existence is the same signal. The owner's pidfd check stays;
both must agree the slot is empty before release.

Release of a contained slot is a short sequence:

1. Close entry. The record moves to a closing state under the
   existing removal journal and fence. After this point the entry
   helper refuses new members for this slot generation.
2. Freeze the slot cgroup (`cgroup.freeze`). Running members stop;
   a member that migrates in despite step 1 arrives frozen and
   flips `populated` back to 1, where step 3 sees it.
3. Read `cgroup.events`. Release needs `populated 0` **and** the
   recorded owner proven dead by pidfd. Read twice, once after the
   freeze and once immediately before deletion.
4. On success, stop the slot unit and record the proof source. On
   any other result, thaw, roll the fence back as today, and
   refuse with the members named (section 6). Nothing is deleted
   on doubt.

If the slot cgroup directory is gone, that counts as released only
when the removal journal already recorded the empty proof and the
unit stop. An unexpected disappearance (for example a
service-manager restart, section 6) is indeterminate and falls
back to the census. Absence alone never proves death; that rule
from #1300 team-pid-namespaces is unchanged.

No step lists `/proc`, walks the slot with `lsof`, or enumerates
cgroup members. Member listing (`cgroup.procs`) appears only in
refusals, to name who blocks release. Its cost is bounded by the
slot's own size.

### What is proven, and what is no longer proven

| Statement | Completed census today | Contained proof |
| --- | --- | --- |
| No contained process uses the slot | Proven at scan instant | Proven while entry stays closed |
| No host process uses the slot by open file, directory, map, or mount | Proven at scan instant | **Not proven**; see section 4 |
| Recorded owner is dead | Proven by generation re-read | Proven by pidfd |
| Slot mount view holds no pins from inside | Partly, by mount checks | Proven: the view dies with its last member |
| Proof finishes on a saturated host | Often false in practice | One file read; section 7 |

Two honest qualifications. First, today's census also has a
residual window: a process that enters after the decisive scan is
unseen, for every slot type. The contained proof replaces that
timing window with a membership rule: after entry closes, arrival
is a rule violation (an escape in reverse), caught by the second
read if it joins the cgroup and named as a blind spot if it does
not. Second, salvage and durability proofs (is this commit safe
elsewhere?) are untouched. This document replaces only the
process-use proof.

Prior art inside this repository: agentctl already reads
`cgroup.events` the same way to decide whether a recorded harness
tree still has processes, in both editions, and treats a malformed
or unreadable file as unknown, never as dead. Notably, it then
also sweeps for escaped members carrying a generation marker,
because its trees must never be forgotten while a member lives.
Section 6 and open question 1 decide whether agent slots need the
same second sweep.

## 2. Entry: who gets in, and how

One helper places processes into a slot cgroup. Every launcher
uses it; there is no second entry path to keep honest. The helper
is the existing slot run/box seam (`wrkslots shell-command` for
panes, `wrkslots run` for commands), extended to place first and
exec second:

- For a process that must keep its own process ID (a pane shell a
  terminal host is already watching), placement uses the user
  manager's transient-unit call that moves the caller without a
  fork. The box already does exactly this, falling back to
  `systemd-run --user --scope` when the call is unavailable.
- For a fresh command, placement is
  `systemd-run --user --scope --collect` with the slot slice named
  as its parent slice. `--collect` matters: finished scopes are
  garbage-collected instead of lingering as empty units (dagrun
  uses it; the gate wrapper notably does not, section 6).
- Where a delegated cgroup directory is available without the
  manager, direct membership writes are allowed, following the
  probe-then-place discipline dagrun uses for containers. This is
  a fallback, not the primary path.

Entry is verified, never assumed. Immediately after placement the
entered process (or its launcher) reads its own cgroup path and
compares it with the recorded slot cgroup chain. A mismatch
refuses the start and names both paths. The box's in-scope entry
already verifies this way; the rule generalizes it. Verification
is what turns "the launcher meant to place it" into evidence the
proof may cite.

**Delegation.** The slot slice itself needs no delegation for the
proof: the manager owns it, wrkslots only reads its files.
Delegation (`Delegate=yes`) is needed only where a member must
create child cgroups of its own, which today means dagrun steps.
Those scopes already delegate, drain their root into a supervisor
child (version 2 forbids processes in a group that has controlled
children), and enable controllers one by one. Under this proposal
they do so inside the slot slice, so every step cgroup is a
descendant the slot proof counts. Requested limits are read back
after placement, as dagrun already does; asking is not proof of
enforcement. Failure to place or delegate where the design
requires it fails closed with a visible warning, following
dagrun's default rather than the gate wrapper's fail-open choice:
a gate may run unbounded rather than not at all (#1254 gate-cpu-containment),
but a slot that claims containment must not.

**Herdr panes.** The terminal server stays outside every slot,
in the initial namespace, reporting host process IDs, exactly as
#1300 team-pid-namespaces decided for its shared server. Entry
rides the pane-shell replacement the launchers already perform, so
the server sees the same pane and the same process lineage.
Nothing about foreground detection changes.

**Harnesses.** Cgroup entry touches no harness sandbox, maps no
user IDs, and starts no launcher step, so it works the same for
Claude Code, Codex, and Muse. That sameness is the point. The
mount sandbox is a different story, recorded in
`common/docs/agentctl/harness-support-matrix.md`: under a
user-namespace box every harness launcher's own sandbox step fails
(it tries to become an inner root the box does not map), Muse has
a supported boxed launch that runs its runtime with its own
sandbox off because the box is the sandbox (#1583 boxed-muse-lane-identity),
Claude Code and Codex have none and use limits-only placement or
the root-helper relay instead (#1569 eden-boxed-lanes-audit). This
proposal therefore makes cgroup entry mandatory for contained
slots and keeps the mount view per-harness and optional. The open
root-helper question (host-routed mount work versus a
helper-private image root) does not block cgroup entry; see open
question 7.

**Validations.** A validation of a contained slot runs inside the
slot slice: the gate's CPU scope (#1254 gate-cpu-containment) and
dagrun's delegated run scope are created under it, so one
`populated` read covers the whole validation. Today those scopes
are siblings, with the gate quota passed to dagrun by environment;
nesting them under the slot slice keeps the quota chain (slice
quotas multiply down the tree) and makes the proof total. If a
host cannot nest them, the validation cgroup is recorded as a
second proof member and release needs both empty.

**Donors and authorized helpers.** A donor enters through the same
helper, with the authorization recorded on the slot record (who
authorized it, which helper identity, until when). An unauthorized
process that merely changes directory into the slot has not
entered; section 4 classifies it as outside access. Donors count
for `populated`, so a forgotten donor blocks release by name.

**Agentcloud workers.** A worker on its own node has no cgroup to
enter. It is recorded as not-a-member (remote), its liveness comes
from its session state, and no host slot proof may be derived from
its absence. A node that runs slots locally applies this same
design locally.

**Children that daemonize.** A child that double-forks, starts a
new session, or closes its terminals stays in the slot cgroup;
membership changes only by an explicit move or a new manager unit.
Dagrun's teardown relies on the same fact: its `cgroup.kill`
reaches escapees that process-group kills miss. A member that
starts its own `systemd-run` scope without naming the slot slice
has escaped (section 6). Slot-aware wrappers (the gate, dagrun,
the entry helper itself) therefore always name the slice, and the
environment handed to members carries the slot cgroup so nested
tooling can nest instead of escaping sideways.

## 3. Mount privacy

**Box view binds** are made inside the slot's mount namespace by
the box. They never exist in the host view, so nothing outside can
pin them, and they vanish when the last member of that namespace
exits. For a contained slot whose users all run in the box view,
an empty slot cgroup also proves the view is gone. The box lets
host mounts appear inside where needed and never sends its own
mounts outward.

**Host image binds** are different: an image-backed slot is
bind-mounted at its slot path in the host view. Any process that
clones a mount namespace while that bind exists (a sandboxed lane,
a service with a private temporary directory restarting every
minute) receives a copy and pins the unmount. Making the bind
private stops later mount and unmount events from propagating,
but copies already handed out still hold the filesystem until
their namespaces die; the existing fence handles those by
replacing the mount-point entry, which the kernel detaches in
every namespace.

This proposal adopts, and does not duplicate, the open fix for
host binds (#1733 wrkslots-sandbox-pins-validate-binds): create
every view of a slot bind private from the start, privatise again
after the fence before unmount, never re-bind a slot whose removal
is journaled or fenced, and migrate existing shared binds at the
next re-establish or remove. The contained proof may skip the
census; it may never skip privatise-and-fence. The companion
diagnostic (#1743 wrkslots-umount-ebusy-name-holder-and-retry),
which names the holding processes and retries only transient
holders by re-checking a condition, stays as the off-hot-path
answer when a pre-privatisation copy still blocks an unmount.
With both landed, the common case is: contained members gone, box
view gone with them, host bind private, unmount succeeds without
asking any foreign namespace for permission.

## 4. Legitimate outside access

Not every touch of a slot comes from inside it. Each kind below is
classified exactly once. "Harmless" needs an argument, not a hope.
"Must-enter" means the access runs through the entry helper or
the slot keeps the census proof. "Gap" means the contained proof
does not cover it; the gap is named in evidence until closed.

| Access | Class | How it is handled |
| --- | --- | --- |
| Coordinator reads (status, logs, salvage inputs) | Harmless | Read-only opens do not block unlink; removal-time reads use the fenced path by the remover itself |
| Shared build caches outside the slot (for example a shared cargo target) | Harmless | Rule: shared writable caches live outside the slot path; a cache inside the slot makes its builder must-enter |
| Object reads from a shared primary checkout (linked-worktree git) | Harmless | Reads only; writes to the slot's own git metadata must-enter or finish before the fence |
| Per-slot build daemon (buck2 for one checkout) | Must-enter | Daemon placed in the slot slice; slot stop asks it to exit, then waits for `populated 0` as a condition |
| File watchers and indexers for the slot | Must-enter | Same daemon rule as above |
| Editors and shells a person points at the slot | Must-enter | Launched through the helper they count as members; ambient access keeps the slot on the census (below) |
| Shared daemon serving several slots | Gap | Cannot be in two slot cgroups; slots it serves refuse census-free release until split per slot |
| Eden daemon and its FUSE mount | Gap, closing | Per-slot daemon gets a slot-slice placement and a stop-before-proof rule (#1423 eden-daemon-containment, #1702 wrkslots-eden-finish-accounting); until then Eden slots stay on the census. Proof code never walks the FUSE mount |
| Backing-store and `sl` writes for Eden slots | Must-enter | In-image backing bound writable only inside the box (#1567 eden-backing-in-image); host backing is never bound writable |
| Image root path that names the subvolume directly | Gap | A same-user process can bypass the slot path (#1143 slots-image-back-door); no cgroup closes this. Named residual, no worse than today's post-scan entrant; closing needs a helper-private image root or a fenced rename |

Two supporting rules. First, a slot may be marked open-access by
an operator (editors and shells come and go freely). An
open-access slot never uses the contained proof; the mark is in
the record and in status output, so the weaker claim is never
made by accident. New contained slots default to closed access.
Second, daemons are the recurring shape of trouble (build,
watcher, Eden): each either lives in exactly one slot cgroup with
a stop rule, lives outside all slots and touches only outside
paths, or keeps every slot it serves on the census. "It usually
exits quickly" is not a class.

## 5. Migration from the census

No flag day. Evidence becomes per-slot data, and behaviour follows
the data:

1. **Record-only first.** The slot record gains the slot-cgroup
   fields and a proof-source field, written in the
   omit-when-absent style Phase 2a used so older readers keep
   working. Status prints the source for every slot (`cgroup`,
   `pidfd`, `scoped-census`, `legacy-scan`). No proof changes yet.
2. **New slots are contained.** Slots created after the change get
   a slot cgroup at creation and enter through the helper.
   Validation slots go first: they are short-lived, disposable,
   and are where the timeouts hurt.
3. **Existing slots adopt at restart.** A running process tree is
   never migrated live: moving members one by one can split the
   tree across two cgroups while children are born, and a split
   proof is worse than an honest legacy one. Adoption happens when
   the lane next starts, as with namespaces. (Unlike a PID
   namespace, a cgroup move is mechanically possible; it is
   rejected here on proof grounds, open question 3.)
4. **The census shrinks last.** A slot drops the host scan only
   when its record proves contained entry, closed access, and no
   open gap from section 4. Everything else (legacy slots, Eden
   slots awaiting daemon placement, open-access slots, any
   unreadable or malformed evidence) keeps today's behaviour
   exactly, including the raise-only operator bound proposed for
   saturated hosts (#1740 wrkslots-validate-batch-census-bound-flag).
   That flag remains useful for the fallback population; this
   design shrinks who needs it, it does not repeal it.
5. **Rollback is stop-recording, keep-reading.** Readers accept
   records with and without the new fields indefinitely. Turning
   the feature off returns every slot to the census; no record
   becomes unreadable and no slot becomes unjudgeable.

Slow removal phases (condition waits, kills, deletion) run
outside the registry lock under the per-slot fence and journal,
extending the narrowing already under way
(#1735 wrkslots-remove-holds-state-lock-through-salvage,
#1213 wrkslots-state-lock-deletion). A contained proof that waits
while holding the global lock would trade one stall for another.

## 6. Failure modes

- **Escape by explicit move or new unit.** Same-user processes can
  write themselves into another cgroup or start a sibling manager
  unit. This design is an accident boundary for cooperating
  tooling, as the box already documents itself, not containment
  against a hostile member. Defences: slot-aware wrappers always
  name the slice; entry verification catches never-entered lanes;
  an operator census remains as an explicit audit diagnostic.
  Whether agent slots also sweep for generation-marked escapees,
  as agentctl does for harness trees, is open question 1.
  Validation slots, whose checkouts are disposable, are proposed
  to trust `populated` alone.
- **Double-fork, new session, closed terminals.** Not escapes.
  Membership is unchanged and `cgroup.kill` still reaches them.
- **Uninterruptible sleep (D-state).** `populated` stays 1 and a
  kill stays pending until the task wakes. Removal waits on the
  condition `populated 0`, names the stuck members from the slot's
  own member list (bounded by slot size), and never judges them
  dead. No current tool handles this case; naming it is new.
- **Leaked and zombie scopes.** Empty units accumulate today
  (section 7 counts thousands). Rules: every entry scope uses
  `--collect`; ownership is the slot record, never a
  working-directory guess; a reaper in the style of dagrun's
  leftover-scope stopper removes empty slot cgroups with no live
  record; `doctor` reports leaked and frozen slot cgroups by
  count and name. A crash after freezing leaves members frozen;
  journal recovery thaws before anything else.
- **User-manager restart.** Transient scopes and their placement
  evidence may vanish while processes survive elsewhere. Any
  recorded cgroup whose boot identity does not match, or that
  disappears without the journal entry from section 1, is
  indeterminate: fall back to the census. Never "released".
- **Eden FUSE.** The daemon, not the checkout walk, is the holder.
  Stop waits on two conditions: the slot's own daemon unit
  inactive and `populated 0`. A wedged FUSE mount is reported,
  never walked.
- **Reboot.** Boot-identity mismatch invalidates all cgroup
  evidence. Slots recover through the existing journals; stale
  cgroup paths judge nothing.
- **No keeper failure.** There is no PID 1 whose exit kills a
  team. The largest new risk of the parked design is absent here;
  in its place stands the escape risk above, which is smaller and
  auditable.

## 7. Cost and performance

The contained proof reads a fixed number of small files: one
`cgroup.events`, one owner pidfd check, one record. Its cost does
not depend on the host process count, the number of other slots,
or the number of files in the slot. The census costs one pass
over every host process (several file opens each) plus a
directory walk of the slot; its batch bound in current code is
120 seconds plus 60 seconds per slot, 540 seconds at eight slots,
and saturated hosts exceed it (#1740 wrkslots-validate-batch-census-bound-flag).

Measured read-only on a loaded multi-team host on 2026-11-16
(load about 95 on 316 hardware threads, about 5,900 processes; no
census or directory walk was run, to avoid adding load):

- One `cgroup.events` read: about 0.03 ms (100 reads).
- Reading `cgroup.events` for 201 scopes: about 4 ms per pass,
  confirmed by a second shell-loop method at the same scale.
- Merely listing process-ID names, with no per-process opens:
  about 32 ms per pass. A real census pays opens per process on
  top of this, under load, which is where its seconds go.
- The user manager held about 3,100 cgroups, about 2,600 of them
  empty, and about 30 leftover run scopes: empty cgroups are
  cheap to read but must still be reaped (section 6).
- Steady cost per slot: one slice, its scopes, a few record fields;
  namespace headroom (millions) does not bind. No idle keeper.

Contained removal waits only for its own members, never for
unrelated work. Tests pin that independence by counting files
opened, never elapsed time.

## 8. Composition with a validation hub lifecycle

Generically, a validation hub's lifecycle is: admit a run in a
fresh checkout, write the exact-commit receipt to its ledger,
then clean the checkout up. Containment slots into it without
reordering anything:

- **Receipt first.** Cleanup eligibility still begins at the
  receipt, never at process exit. Removal consumes the receipt
  plus the slot proof; it re-derives no verdict. A faster proof
  shortens cleanup only.
- **Cleanup-pending backpressure, bound K.** At most K checkouts
  may sit cleanup-pending (receipt written, checkout present).
  Past K, new validations queue or refuse. Contained slots drain
  in one condition wait, so K binds only the hard residue
  (escapes, stuck tasks, gap slots on the census), which then
  appears as a visible queue instead of silent disk growth.
- **One sweeper, one identity.** A single named per-host service
  owns cleanup-pending retries for hub slots. It runs outside
  every slot cgroup, closes entry, waits for `populated 0`, falls
  back to the census only for legacy or gap slots, and records
  the proof source per slot. One identity means one audit trail
  and no competing removers; its lock use follows section 5.

## 9. Phased plan and open questions

Each phase lands alone, in both editions where the surface exists;
where it does not (the census commands are Python-only today),
the other edition refuses to it and the gap is recorded. Every
test waits on a condition (a file's content, a barrier, a recorded
state), never on a wall-clock budget; cost tests assert counts of
files opened or members read. Fake cgroup trees follow the
hermetic pattern of #1482 census-gone-tests-hermetic.

0. **Record and status only.** Accept: old records read unchanged;
   new fields omitted when absent; status names the proof source.
1. **Contained validation slots.** Slice at creation, verified
   entry, `populated` liveness. Accept: fake-tree populated
   1/0/malformed/missing give live/releasable/unknown/unknown;
   entry mismatch refuses naming both paths.
2. **Census-free validation removal.** Accept: a slot member
   blocks by name; thousands of unrelated fake processes change
   neither verdict nor files-opened count; a stuck member is
   named, never deleted under; fallback slots behave byte-same.
3. **Agent lanes and panes.** Accept: pane entry preserves the
   watched process ID under stubbed placement; all three harnesses
   enter with no mount view; stop waits on the empty condition.
4. **Daemon placement.** Per-slot build, watcher, and Eden
   daemons in the slot slice (#1423 eden-daemon-containment).
   Accept: a shared daemon refuses census-free release naming the
   gap; a placed daemon's exit is the waited condition.
5. **Mount privacy completion**, on top of
   #1733 wrkslots-sandbox-pins-validate-binds and
   #1743 wrkslots-umount-ebusy-name-holder-and-retry. Accept:
   stubbed-runner fixtures show private binds on every view and
   clean removal with a foreign copy present.
6. **Optional per-run PID namespaces** for boxed validations
   whose launchers allow them. Accept: sibling runs are invisible
   to each other in a scratch-namespace fixture; limits unchanged.

Decisions only the owner can take:

1. Agent slots: `populated` alone, or plus an escape sweep?
2. New agent slots default to closed access (ambient use means census)?
3. Adoption at restart only, live migration never?
4. Freeze during the release proof, or close-entry without freeze?
5. Eden containment waits for per-slot daemon placement; how urgent?
6. PID namespaces stay per-run only, retiring the per-team keeper?
7. Image mounts: keep host-routed helper operations, or pursue a helper-private image root?
8. Cleanup bound K and sweeper ownership: hub config, or wrkslots default?
