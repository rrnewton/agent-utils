# Team PID namespaces: slot liveness without host-wide scans (#1300 team-pid-namespaces)

Status: Phase 1 design, 2026-11-12; Phase 2a implemented (see below).
Phase 2b (the keeper and the join) is not yet authorized: it waits for the
launcher-compatibility matrix named in the Phase 2b note below. Interfaces
described for Phase 2b must not be read as shipped.

## Goal

Each agent team (a coordinator plus its workers) runs in its own PID
namespace, so slot liveness and removal-safety evidence enumerate only the
team's processes instead of scanning host-wide `/proc`.

## Success criteria

These restate the acceptance criteria of #1300 team-pid-namespaces:

- A namespaced slot reports liveness from its recorded owner (pidfd, cgroup,
  or namespace evidence, named in status) with no host-wide walk on the
  heartbeat, status, or remove path; census cost is independent of the host
  process count.
- Removal-safety evidence for a namespaced slot enumerates only its
  cgroup/namespace members; a process outside the team that uses the slot
  path through another mount is still detected by the existing path/mount
  checks, or the refusal names the blind spot.
- A boxed validation run sees only its own PID tree while keeping today's
  cgroup limits; sibling validations are invisible to it.
- Pre-namespace slots keep today's behaviour until restarted, status names
  the evidence source per slot, and no slot is judged dead solely because
  namespace evidence is absent.
- Remaining host-wide scans are listed as operator diagnostics with their
  scope stated; none runs on create/remove/heartbeat paths.

## 1. What exists today (evidence for the design)

- Slot identity is `ProcessIdentity { pid, start_ticks, boot_id, host_id,
  cgroup_path }` in `py/wrkslots/cli.py`, with no namespace field. Liveness
  (`_process_state`) re-reads that generation from host `/proc`; a PID reused
  or a boot change reads as dead, anything unreadable as indeterminate.
- Removal safety still enumerates the host: `_assert_slot_unused` and the
  absent-row census (`_absent_validate_process_snapshot`) walk every
  `/proc/<pid>` mount table, cwd, root, and descriptor set, under host-scaled
  CPU/output bounds that exist because a census on a busy host took tens of
  seconds. A bounded cgroup alternative already exists for one question:
  `_retained_unit_cgroup_members` counts members from `cgroup.procs` under
  the user manager after proving the whole cgroup v2 hierarchy is visible
  (`_assert_whole_cgroup2_hierarchy`).
- The code already distrusts a foreign PID view for host proofs:
  `_assert_host_process_view` and `_in_initial_pid_namespace` refuse a
  non-initial PID namespace (initial inode `pid:[4026531836]`) when a missing
  process would be read as dead, and the owner-identity guard explains at
  length why a namespace-local PID 1 recorded from inside its own namespace
  resolves to dead from the host rather than sticking. Any namespace design
  must preserve that asymmetry: host-side provers stay in the initial
  namespace.
- The box (`py/wrkslots/sandbox.py`) already enters an unprivileged user and
  mount namespace in-process: `enter_userns` unshares
  `CLONE_NEWUSER | CLONE_NEWNS`, writes the uid/gid maps, builds the view,
  and execs, so the command keeps the caller's PID. It creates no PID
  namespace, and the `root` mode's comment states why: harness launchers
  that talk to the user's systemd manager fail inside a PID namespace,
  because the bus cannot identify a peer in another PID namespace (the
  kernel reports such a peer's PID as 0; `py/agentctl/pidns.py` uses exactly
  that `SO_PEERCRED` test to classify a Herdr server as foreign).
- Panes are spawned by the Herdr server, not by agentctl: agentctl asks
  wrkslots for an exec line (`wrkslots shell-command SLOT`, via
  `_slot_shell_command` in `py/agentctl/subagents.py`) and the pane shell is
  replaced by that line before the harness starts. `agentctl restore` and
  `start` in the shipping edition are the Rust edition's commands
  (`rs/agentctl/src/restore.rs`); the Python edition refuses them. Restore
  revives the coordinator first from its launch spec, then each worker, and
  starts configured units last; the Herdr server itself is a separate user
  unit (`herdr-server.service`, `KillMode=process`) shared by every team of
  the user.
- Gates are not services: `scripts/gate.py` wraps validation in
  `systemd-run --user --scope` with only a `CPUQuota` property
  (`_contained_command`), inside which dagrun creates its own per-step
  cgroups (`py/dagrun/cgroup.py`).

## 2. Scratch experiment (Phase 1 proof, run on the target host class)

Tiny scratch probes only; no registry, slot, or shared state was touched.
Commands were bounded with `timeout`; no result depends on a wall-clock wait.

| # | Probe | Observed result |
|---|---|---|
| 1 | `unshare --user --map-root-user --pid --fork --mount-proc sh` | Inner shell is PID 1; `/proc` has 4 numeric entries; the host view at the same moment has 4,667 |
| 2 | Same, without `--mount-proc` | Inner shell is PID 1 but `/proc` still lists 4,673 entries: the inherited host procfs silently defeats the namespace |
| 3 | `unshare --pid --fork` with no user namespace | `Operation not permitted`: a PID namespace alone is not unprivileged on this host; the user namespace is what grants it |
| 4 | Keeper in a namespace (probe 1 form, idling), then `nsenter -t KEEPER -U --preserve-credentials -p -m -F` plus a fresh proc mount in the child | Join succeeds; the joined `/proc` has 3 numeric entries. Without the mount-namespace step and fresh proc mount, the joiner still sees 4,621 host entries |
| 5 | `systemd-run --user --scope -p PrivatePIDs=yes` | Refused: `Unknown assignment: PrivatePIDs` — a scope cannot take it on this systemd (261) |
| 6 | `systemd-run --user --wait --pipe -p PrivatePIDs=yes` (transient service) | Works: a fresh PID-namespace inode distinct from the initial one, `/proc` has 3 numeric entries |
| 7 | Plain `nsenter -t KEEPER -p` join of the probe-6 service's namespace | `Operation not permitted`; entering its user namespace first (`-U --preserve-credentials`) makes `setns` succeed, but the joined process's `/proc` view and uid mapping were not verified in Phase 1 |

Conclusions the rest of the design relies on:

1. The unprivileged primitive that works is a **user namespace owning a PID
   namespace**, entered exactly as the existing box enters its user
   namespace, plus `CLONE_NEWPID` and a fresh procfs mount. Probe 2 is the
   failure mode to design against: a PID namespace without a remounted
   `/proc` is indistinguishable from no namespace to every `/proc` reader.
2. A PID namespace takes effect for **children, never for the process that
   creates or enters it**. Creation therefore needs fork-then-exec (the
   box's in-process `unshare` + `exec` pattern works because `exec` is the
   child step that matters), and joining needs fork-after-`setns` — the
   `nsenter -F` shape — never an in-place `setns` in a long-lived process.
3. The systemd route is narrower than it looks: `PrivatePIDs` exists only
   on a **service**, not on the scopes gates and slots use today (probe 5),
   and a service-owned namespace is not plainly joinable by the team's later
   processes (probe 7). systemd can supervise the keeper (section 3), but
   it cannot be the mechanism by which panes join.

## 3. How a team gets its namespace

**Decision: a per-team namespace keeper, owned by a user namespace the team
creates itself, supervised as a transient user service.** Not a systemd
scope with `PrivatePIDs` (probe 5 refuses it), and not a `PrivatePIDs`
service as the namespace owner (probe 7: its namespace is not plainly
joinable, which is the property the whole design needs).

Mechanics, in restore order:

1. `agentctl restore` (Rust edition, `rs/agentctl/src/restore.rs`) gains a
   step before the Herdr check: ensure the team keeper. The keeper is one
   minimal process that does nothing but hold the namespace open: it enters
   a new user + PID + mount namespace (the `enter_userns` sequence in
   `py/wrkslots/sandbox.py` extended with `CLONE_NEWPID`, written once in
   the edition that owns restore), mounts a fresh `proc` on `/proc`, and
   then idles as PID 1 of the new namespace. The existing stress harness
   already runs whole suites under this shape (`unshare --pid --fork
   --mount-proc`), so the pattern is exercised, not novel.
2. The keeper is started as a transient user **service** (for example
   `agentctl-team-<team>.service`, `Restart=on-failure`), never a scope.
   systemd then owns exactly the jobs it is good at: a cgroup for the team
   (section 5), supervision, and a unit an operator can inspect. The
   service's own cgroup is the team cgroup; the namespace lives inside it
   because the keeper created it, not because the unit asked systemd for
   one. No `PrivatePIDs` property is set.
3. Before reviving any agent, restore records the team namespace in the
   registry, beside the existing per-agent records: the PID-namespace
   inode (`/proc/<keeper-host-pid>/ns/pid`, a `pid:[...]` string), the
   user-namespace inode, the keeper's host PID generation
   (`{ pid, start_ticks, boot_id }` — the same triple `ProcessIdentity`
   already trusts), and the keeper unit name. This record is the team's
   root of namespace evidence; every later join and census cites it.
4. `agentctl start` of a coordinator outside restore (a cold team with no
   keeper) performs the same ensure-keeper step lazily. There is one code
   path for "the team's namespace exists and is recorded"; restore and
   start both call it. A team that cannot create the namespace (probe 3's
   host policy, a missing `unshare` primitive, a refused keeper unit) is
   recorded as `namespace: unavailable` with the reason, and the team runs
   exactly as today (section 7). Unavailability is a recorded state, never
   a failed start.

Why the coordinator is not PID 1: if the namespace init exits, the kernel
kills every process in the namespace. Making the coordinator the init
would let one agent's exit destroy its whole team; a dedicated keeper
makes team lifetime an explicit, supervised decision instead. The keeper
must therefore be the most boring process in the system: no harness, no
network, no writable state beyond its registry record, and nothing to do
that could crash it. Its death is still catastrophic for the team
(section 8), so restore treats a dead keeper with live recorded members
as a refused restore that names the namespace loss, never as a silent
re-creation — a new namespace under the old name would strand every
recorded namespace-PID in the registry.

Alternatives rejected:

- **Systemd user scope with `PrivatePIDs`** — disproved by probe 5 on this
  systemd; scopes take no such property. Even where a newer systemd
  accepts it, a scope is created per command and dies with it, so it
  cannot be the long-lived team container panes join.
- **`PrivatePIDs` service as namespace owner** — creates a namespace
  (probe 6) but probe 7 shows the team's ordinary processes cannot join
  it without first entering its user namespace, whose uid mapping and
  `/proc` view Phase 1 did not verify; and each additional service would
  get its own namespace, not the team's. Phase 2 may re-probe it as a
  keeper implementation detail (the keeper itself is one service either
  way), but the join contract must be designed against the proven
  `setns`-into-a-user-namespace shape, not against this one.
- **Coordinator as namespace init** — rejected above: couples team
  lifetime to the most crash-prone process in the team.
- **One Herdr server per team, server inside the namespace** — section 6.

## 4. How lanes join it

A lane (worker, or the coordinator at revive) joins at pane-shell
replacement time, through the seam that already exists:

1. `agentctl start --slot` already replaces the pane shell with the line
   `wrkslots shell-command SLOT` prints, and the box preserves the caller's
   PID across `exec` so Herdr still sees the agent where it expects. The
   namespace join rides that line: when the team's record names a live
   keeper, the shell-command gains a join prefix that (a) opens the
   keeper's user and PID namespaces, (b) `setns` into the user namespace
   first, then the PID namespace, (c) forks — the PID namespace applies
   only to the child (probe 4) — (d) mounts a fresh `proc`, and (e) execs
   the existing box entry. The box then builds its mount view exactly as
   today, in a mount namespace derived inside the team's namespaces.
   Joining before the box entry keeps one ordering invariant: nothing of
   the lane exists in the host namespace except the already-spawned pane
   shell, which is replaced by the join.
2. Headless workers and non-slot lanes join by the same prefix around
   their launcher command; there is no second join implementation. A lane
   whose launch path cannot take the prefix (a foreign or adopted pane,
   agentcloud — section 6) is recorded `namespace: not-a-member` with the
   reason, and stays fully supported under the fallback.
3. Join is verified, not assumed: immediately after exec, the lane's
   startup records its namespace-observed identity — `/proc/self/ns/pid`
   compared against the team record, and its in-namespace PID — in its
   agent record. A mismatch (the pane shell ignored the prefix, an old
   shell-command line was replayed) refuses the start with the two inodes
   named, rather than running a supposedly namespaced lane on the host
   view. This is the probe-2 guard made permanent.
4. Slot ownership binding (`_bind_launched_slot_owner`,
   `wrkslots transfer-owner`) is unchanged in shape but gains the
   namespace dimension: the owner identity recorded for a slot is the
   **host** PID generation read by the host-side binder, plus the
   in-namespace PID and namespace inode the lane reported. Recording only
   the in-namespace PID is the known degenerate case (a namespace PID 1
   recorded as host PID 1) that the existing guard already documents;
   recording only the host PID loses the ability to enumerate the team
   from inside. Both numbers, with the inode, are the identity.

## 5. How wrkslots liveness and census use the namespace

Three evidence sources, in preference order, each named in status output
(`evidence: pidfd | cgroup | namespace | legacy-scan`). No path judges a
slot dead from the *absence* of namespace evidence.

1. **Owner pidfd (the heartbeat/status fast path).** Liveness of a slot is
   a question about one recorded process, not about the host. The checker
   opens a pidfd on the recorded owner's **host** PID, reads the generation
   (`start_ticks`, `boot_id`) after opening it — the ordering the held-lock
   proof in `py/wrkslots/cli.py` already uses — and answers live/dead from
   that one process. Cost is O(1) in the host process count, it works from
   the initial namespace regardless of which namespace the owner lives in,
   and PID reuse cannot redirect it. This replaces `_process_state`'s
   host-`/proc` re-read for every slot that has a recorded owner, which is
   every slot created through the current code. Where this answer exists,
   no census of any scope runs on heartbeat, status, or remove.
2. **Team cgroup membership (the census, when a census is truly needed).**
   Removal safety asks a wider question — is anything else still using
   this checkout — and for a namespaced slot the honest scope for that
   question is the team, not the host. The keeper's service cgroup (section
   3) is the enumeration root: read `cgroup.procs` for that cgroup and its
   descendants (the `_retained_unit_cgroup_members` pattern, including its
   whole-hierarchy visibility proof), then apply the **existing** direct-use
   checks (cwd, root, exe, descriptors, mounts naming the slot path) to
   exactly those members. Evidence states its scope in so many words:
   `scope: team-cgroup <unit>, N members`, never a bare count. Cost is
   O(team size), independent of the host.
3. **Namespace `/proc` view (inside-the-team checks).** A checker that
   itself runs inside the team (a boxed command, an in-namespace health
   probe) reads its own remounted `/proc`, which probe 1 shows contains
   only team processes. This view is authoritative for membership ("is
   this PID one of ours") and useless for host-PID proofs — a checker in
   this view must never feed `_process_state`-style host generation
   comparisons. The existing `_assert_host_process_view` refusal is the
   guard that keeps the two views from being confused, and it stays: any
   code path that would prove death from absence first proves it is
   looking at the namespace that owns the question.

The outside-the-team blind spot, stated plainly: a process in another
team (or no team) that opens the slot path **through its own mount
namespace** is not in the team cgroup and not in the team `/proc`, so
sources 2 and 3 cannot see it. That is the acceptance criterion's second
bullet, and the design does not pretend the namespace answers it.
Removal for a namespaced slot therefore keeps, unchanged, the checks
that do not depend on process enumeration scope: the mount-table and
path-identity checks over the slot's own mounts, the handoff/hold guards,
and the registered-liveness authority. Where a removal relied *solely*
on the host census to exclude outside users, the refusal must now name
the blind spot (`outside-team users of this path are not enumerated;
evidence scope was the team`) rather than silently narrowing. Phase 2
enumerates those call sites explicitly; any it cannot re-ground in a
scope-independent check keeps the legacy scan for that slot (source:
`legacy-scan`, named in evidence) until it can.

**Fallback when not namespaced.** A slot with no namespace evidence —
pre-migration, `namespace: unavailable`, a `cgroup`-isolation lane whose
record says why it could not join — uses today's behaviour exactly:
recorded-owner generation comparison and, where required, the host
census, with `evidence: legacy-scan` in status. The cgroup-only box mode
records why it cannot provide a namespace view (its processes stay in
the host PID namespace by design), and such slots are first-class
citizens of the fallback, not errors. Fallback is per-slot, decided from
that slot's record, never from a global flag.

**Validations.** A boxed validation already gets its own cgroup tree
under dagrun; Phase 2 adds a per-run PID namespace to the boxed modes
(`userns` gains `CLONE_NEWPID` + fresh proc, as in probe 1) so the run's
init is PID 1 of its own namespace and sibling validations are invisible
to it, while `cgroup` mode keeps host visibility and records that it
does. Gate scopes (`systemd-run --user --scope`, CPU quota only) need no
change: the namespace is created inside the scope by the box, and probe 5
is why nobody should try to ask the scope for one.

## 6. Herdr panes, agentcloud, and gates

**Herdr owns spawning; the team owns the namespace; neither owns the
other's lifetime.** The Herdr server is one user unit shared by every
team, started before any team exists, with `KillMode=process` so its
restart does not kill agents. Putting the server inside one team's
namespace is therefore impossible without a server per team — rejected:
it would fragment the shared layout, multiply servers, and make every
cross-team pane operation a cross-namespace translation of the kind
`py/agentctl/pidns.py` exists to refuse-or-translate carefully. Instead:

- The server stays in the initial namespace and reports host PIDs, which
  remain true: a namespaced lane has a host PID as well as a namespace
  PID, and Herdr's PID names the host one. agentctl's existing identity
  proofs (pane foreground = recorded host generation) keep working
  unchanged, because they were always host-namespace statements.
- The join happens in the pane shell agentctl already replaces (section
  4), so from Herdr's perspective nothing changes: same pane, same shell
  PID lineage, same foreground detection. The one new obligation is
  agentctl's: a record must never compare Herdr's host PID against the
  lane's in-namespace PID. The dual recording in section 4 is what makes
  that confusion structurally impossible rather than a comment.
- A Herdr server restart does not touch the keeper (a separate unit), and
  a keeper restart is refused while members live (section 3), so the two
  lifetimes never cascade silently. Panes whose lane died with a lost
  namespace are ordinary dead lanes to revive.

**agentcloud agents** run on their own node under a session the host does
not spawn; there is no host PID namespace to join and no host `/proc`
question to answer. They are recorded `namespace: not-a-member
(remote)`, their liveness continues to come from the agentcloud session
state, and no slot on the host may derive a live/dead answer about them
from namespace absence. If a future agentcloud node runs teams locally,
it adopts this same keeper design on that node; nothing host-side
changes.

**Gates** keep their scopes and quotas. A gate is a validation with more
ceremony: inside its scope, the boxed run gets its per-run namespace
(section 5), so a gate's process evidence stops paying for the host —
the census-under-gate-load failures that motivated #1300 — while the
scope's CPU containment (`_contained_command`) is untouched. A gate run
*about* namespaced slots runs its wrkslots checks from the initial
namespace, under the `_assert_host_process_view` discipline above.

## 7. Migration, with no flag day

1. **Record-only first.** The registry learns the new fields (team
   namespace record, per-slot `evidence` source, dual owner identity) and
   status learns to print them. No behaviour changes; every existing slot
   reports `evidence: legacy-scan` or its pidfd/cgroup equivalent, and
   the recorded-owner pidfd path (source 1) can be switched on first
   because it needs no namespace at all.
2. **New teams get keepers.** `restore`/`start` ensure a keeper for teams
   created after the change; their new lanes join and report
   `evidence: namespace`/`cgroup`. Existing teams are untouched.
3. **Existing teams adopt at their next restore.** A restore that finds
   a team record without a namespace creates the keeper, then revives
   lanes through the join path; a lane revived before its team's keeper
   exists (an opportunistic single-lane revive) simply stays on fallback
   until its next start. Running lanes are never migrated in place — a
   process cannot change its PID namespace — and nothing pretends
   otherwise; adoption is always at (re)start.
4. **Legacy scans shrink last.** Only when a slot's record proves it has
   answered from pidfd/cgroup/namespace evidence do its hot paths drop
   the host walk. Host-wide scans that remain are re-homed as explicit
   operator diagnostics (host audit, cross-team doctor) with their scope
   printed, per the acceptance criteria. Rollback at every step is
   "stop recording, keep reading": readers accept records with and
   without namespace fields indefinitely, and no step makes a
   pre-namespace record unreadable or a slot unjudgeable.

## 8. Risks

- **Keeper death kills the team.** Kernel semantics: the namespace init
  exiting SIGKILLs every member. Mitigations are the boring keeper, its
  supervised service, and the refused-restore rule — but a keeper OOM or
  host-level kill is still a team-wide outage that cannot happen today.
  This is the design's largest new risk and the first thing Phase 2 must
  make observable (keeper health in status, an alarm on keeper exit).
- **Stale-`/proc` false confidence** (probe 2): a join that forgets the
  fresh proc mount sees the whole host and every namespace guarantee
  silently inverts. The post-join inode verification (section 4) exists
  for this; a test must pin it.
- **PID-namespace confusion in identity proofs.** The codebase already
  carries scar tissue here (the PID-1 degenerate-owner analysis, the
  foreign-server translation refusals). Dual recording plus keeping all
  death-proofs in the initial namespace is the mitigation; any Phase 2
  code that records a PID without naming its namespace is a defect by
  definition.
- **The systemd-manager blind spot.** Harness launchers that talk to the
  user manager fail inside a PID namespace (the bus sees peer PID 0).
  Lanes whose launcher needs the manager must either do that step before
  the join or stay on `cgroup` fallback; Phase 2 inventories launchers
  instead of discovering this lane by lane in production.
- **Outside-team blind spot on removal** (section 5): narrowing the
  census narrows what removal can exclude. The scope-independent checks
  and the named-blind-spot refusal are the mitigation; the failure mode
  to avoid is a removal that reports host-grade certainty from
  team-grade evidence.
- **Namespace exhaustion and host policy.** Each team costs one user +
  PID namespace and one idle process. The observed host limit is ample
  (`max_user_namespaces` in the millions), but a host that disables
  unprivileged user namespaces degrades every team to fallback at once —
  recorded, supported, and alarmed, not fatal.
- **Mixed-version readers.** Older agentctl/wrkslots copies read the same
  registry during rollout (the layout-2 migration is the precedent).
  New fields must be ignorable by old readers and old records readable
  by new readers; a reader that cannot parse the namespace record must
  fall back, never refuse the registry.

## Phase 2a (implemented): migration step 1, no namespaces

- `ProcessIdentity` gained the dual fields `namespace_pid` and
  `pidns_inode` in both wrkslots editions. They are optional on read,
  preserved when present, omitted on write when absent, and excluded
  from generation equality, so a pre-namespace record compares equal to
  a current reading of the same generation. Capture does not populate
  them yet: a stored record carrying new keys would be refused by an
  older reader that knows only the five keys (an existing older-client
  test pins exactly that), so the Phase 2b join is what first records
  them.
- Owner liveness answers pidfd-first (`_process_state_with_evidence` /
  `process_state_with_evidence`): the pidfd pins the generation, and the
  single-process generation read that follows cannot be redirected by
  the exit/fork races of a host walk. When a pidfd cannot be opened for
  a reason other than absence, the legacy reading answers and status
  names the source: `owner_evidence: pidfd | legacy-scan | none`.
- The removal census for a slot whose recorded owner cgroup is
  slot-scoped (`wrkslots-…`) enumerates only that cgroup's members
  (`_cgroup_member_pids`, `_scoped_cgroup_slot_users`) and applies the
  direct-use checks to them; when the scoped census cannot run, the
  legacy host census answers unchanged. Slots without a slot-scoped
  cgroup keep the legacy census exactly as before.
- Tests assert counts and evidence strings, never wall time: pidfd
  liveness with zero host `/proc` listings, scoped-census membership
  independent of other cgroups' process counts, and pre-namespace
  records answering unchanged, in both editions.

## Phase 2 sketch (not authorized by this document)

Keeper + registry record in the Rust edition's restore/start; the join
prefix in `shell-command`; `ProcessIdentity` gaining the namespace pair
in both wrkslots editions with the census evidence source named in
status; per-run namespaces in the boxed validation modes; and the test
set the acceptance criteria name — O(1) census cost against thousands of
unrelated host processes, sibling-validation invisibility, the
stale-`/proc` join test, and pre-namespace slots answering unchanged.
No wall-clock waits in tests; bounds are asserted on process counts and
evidence strings, not on elapsed time.
