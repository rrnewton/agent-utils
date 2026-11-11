# wrkslots Eden checkouts: Phase 1 survey and design

Issue: #1204 wrkslots-eden-checkouts. Phase 1 is read-only survey and
design; no Eden checkout is created or removed until Phase 2, with
coordinator approval, on a slot the owner frees. All commands below were
run read-only on 2026-11-10 (ET).

## Survey: Eden on this host

- One daemon (pid 91202 at survey time) serves every checkout:
  `edenfs --edenDir /home/newton/local/.eden`. `eden list` shows 3
  checkouts, which is already the owner's per-host cap of 3.
- `eden config` reports `edenDirectory = "${HOME}/local/.eden"`. Client
  state lives in `~/local/.eden/clients/<name>/` (`config.toml`,
  `SNAPSHOT`, `sl-repo-dir`, and `local/`, the sharded overlay store;
  about 15G for the largest client). Backing stores are shared under
  `~/local/.eden-backing-repos/` (about 9.9G). `eden info <mount>` gives
  `state_dir`, `backing_repo`, `checked_out_revision` and
  `working_copy_parent` per checkout; all three share one backing repo.
- `eden clone <backing-repo> <path>` takes the backing repository as a
  positional argument (there is no `--backing-repo` flag); options are
  `--rev`, `--backing-store` (`filteredhg`, `recas`, `http`),
  `--filter-paths`, `--overlay-type`. The global `--config-dir` (or
  `$EDENFSCTL_CONFIG_DIR`) relocates all daemon state; the default is
  the `edenDirectory` above. A non-default config dir means a separate
  daemon for that state.
- `eden remove [-y] [--timeout SECONDS] <path>...` unmounts and deletes
  client state. It does not delete buck-out redirection targets: after
  past removals the targets stayed behind as orphaned redirections
  (the owner once found about 835 GB orphaned on /data).
- Redirections: `eden redirect list --mount <mount> [--json]` shows
  `buck-out  bind  /data/users/<user>/scratch/homeZ...Z<mount>/edenfsZredirectionsZbuck-out`
  with source `.eden-redirections` (the in-checkout file that declares
  `"buck-out" = "bind"`). The daemon derives the target from the mount
  path under the scratch directory on /data's main filesystem, and
  bind-mounts it over `<mount>/buck-out`. `eden redirect add <repo-path>
  <bind|symlink> --mount <mount>` declares a redirection but takes no
  target argument. Live mounts still show `//deleted` bind entries for
  removed checkouts, confirming that remove leaves them behind.
- `eden du [--fast|--json|--clean|--clean-orphaned] <mount>` is the
  only safe usage meter (never `du` a mount). Survey values for the
  largest checkout: materialized 15.7 GB, orphaned redirections
  4.1 GB, backing repos 9.9 GB. `eden du --clean-orphaned` reclaims
  orphaned redirection space. `eden stats` reports daemon memory
  (8.3 GB private) and per-mount inode counts.

## Survey: wrkslots on this host

- Config (`.wrkslots.yml`): flat layout, `slot_representation:
  worktree` for new slots, and a `worktrees_image` section: one sparse
  btrfs image `worktrees.img` (2 TiB apparent, about 141 GiB
  allocated), kernel loop mount at `worktrees/.slots-image`, one
  subvolume per slot bind-mounted at `worktrees/slots/<slot>`,
  `slot_quota_bytes` 256 GiB per slot. `btrfs subvolume list` shows
  one subvolume per slot; `btrfs qgroup show` reports referenced bytes
  per slot. Per-slot ext4 images (`slotimage.py`) exist but no slot
  uses them here.
- Slot model (`py/wrkslots/cli.py`): an `ActiveRecord` binds slot,
  agent, task, generation, owner/coordinator process identities,
  heartbeat, handoff, and a tuple of `Checkout` rows. A `Checkout`
  already carries `vcs` (default `"git"`), name, path, repository,
  branch, start point, remote, landed ref and HEAD. Slot types are
  `agent` (salvage before reclaim) and `validate` (never salvaged).
- Lifecycle: `create` makes linked git worktrees and registers in one
  journaled operation; `register` and `import-existing` (dry-run by
  default) adopt existing trees; `hold`/`unhold` protect a slot from
  cache cleanup and removal; `audit` gives DELETABLE/BLOCKED/HELD
  verdicts; `remove` salvages agent slots to an allowed remote or a
  verified local bundle under the control directory, then runs the
  process census (cwd, executable, file descriptors, memory maps,
  mount references, recorded cgroup) plus liveness, TTL, Git and path
  checks, and refuses on any disagreement. Removal is journaled and
  resumable with `recover`.
- btrfs baseline at survey time: /data (the host filesystem holding
  `~/local/.eden`, the backing repos and every scratch redirection
  target) reports Metadata,DUP Size 232.59 GiB, Used 105.93 GiB. The
  slots image reports Metadata,single Size 2.01 GiB, Used 1.35 GiB.
  Freed btrfs metadata is not free space, so the measurement plan
  below tracks Used and allocated separately, never `df` alone.

## What "inside the image" must mean

An Eden checkout has four storage pieces, and only the first sits at
the mount path by default:

| Piece | Default home | Inside image requires |
|---|---|---|
| Mount (working copy view) | wherever `eden clone` puts it | mount path inside the slot subvolume |
| Client state + overlay | `~/local/.eden/clients/<name>/` on /data | per-slot `--config-dir` inside the subvolume |
| Backing repo (shared store) | `~/local/.eden-backing-repos/` on /data | left shared outside (see below) |
| buck-out target | scratch dir on /data, bind-mounted in | target directory inside the subvolume |

Recommendation: mount path and client state go inside the slot
subvolume; the backing repo stays shared outside. The backing repo is
a shared, read-mostly store whose cost is fetches, not per-checkout
overlay metadata; cloning it per slot would multiply 9.9G and lose
sharing. The cost of a per-slot config dir is a separate Eden daemon
per Eden slot (the shared daemon already holds 8.3 GB private), so
the design caps Eden slots by the host cap anyway (3) and Phase 2
must measure daemon memory per added slot. Keeping the shared daemon
and only moving the mount would leave overlay metadata on /data and
defeat the owner's experiment, so it is not recommended except as a
measured control arm.

Open mechanism for buck-out: `eden redirect add` names no target, so
wrkslots cannot yet tell Eden to use an in-image target directly.
Phase 2 must determine, on the trial slot, which of these the daemon
honours: (a) a `symlink` redirection to an in-image directory,
(b) pre-binding an in-image directory over the derived scratch
target before first use, recorded by wrkslots and verified after,
or (c) a daemon/config setting for the redirection base. Until one
is proven, create must refuse rather than silently leave buck-out
on /data.

## Design

### Slot model

Add a second checkout kind beside git worktrees by using the
existing `Checkout.vcs` field with value `eden`. An Eden checkout
row records: name, mount path (inside the slot), backing repo path,
checked-out revision, client state directory (inside the slot),
config dir, and redirection targets with their in-image paths. A
slot holds either git checkouts or one Eden checkout, never both;
flat layout makes the Eden mount a child of the slot subvolume
(e.g. `<slot>/checkout`), never the subvolume root itself, so the
FUSE mount never stacks directly on the bind mount point. New CLI
surface mirrors `create`: `wrkslots create SLOT --eden
NAME=BACKING_REPO [--eden-rev REV]`; `register`/`import-existing`
learn to verify an existing Eden mount (`eden info` state_dir and
revision must match the row) instead of git worktrees.

### Create flow

1. Refuse unless the worktrees image is verifiably mounted and the
   host-wide Eden count (below) is under the cap.
2. Create the slot subvolume with its 256 GiB qgroup, as today.
3. `eden clone --config-dir <slot>/.eden-state <backing-repo>
   <slot>/checkout` (plus `--rev` when given), so overlay and
   client state are born inside the image.
4. Establish the buck-out redirection with its target inside the
   slot (mechanism per the Phase 2 finding), then verify:
   `eden redirect list --json` target is inside the slot subvolume
   and mountinfo agrees.
5. Register the slot row and run post-provision hooks, journaled
   exactly like git create; any failure leaves the journal for
   `recover`, never a half-registered slot.

### Cap enforcement

The cap counts every Eden checkout on the host, not just wrkslots
slots: the union of `eden list` for the shared config dir and for
every wrkslots Eden slot's config dir, deduplicated by mount path.
Create checks the count under the registry lock immediately before
cloning and refuses at 3. `status`/`audit` report the host count
and warn when foreign checkouts alone reach the cap. The count is
read from the daemons, never from directory guesses, and a daemon
that cannot be queried refuses creation (fail closed).

### Remove flow (redirection cleanup verified)

1. Holds refuse removal, as today. The process census extends to
   the Eden mount path, its state dir and every redirection target.
2. Salvage first (below) for agent slots.
3. Record redirection targets from `eden redirect list --json`.
4. `eden remove -y <mount>`, then explicitly delete each recorded
   target directory and the slot's client state.
5. Verify, and refuse to delete the subvolume until all hold:
   the mount is gone from `eden list` and mountinfo, no bind mount
   references a recorded target, every target path is absent, and
   `eden du --json` for remaining checkouts shows no new orphaned
   redirections attributable to this slot. Only then delete the
   subvolume and archive the row. A failed verification preserves
   the subvolume and says exactly what remains.

### Salvage

Eden checkouts are Sapling working copies. Before remove, run
`sl -R <mount> status` and compare `sl id` with the recorded
revision. Any modification, or local draft commits, must be
preserved as a patch plus a bundle of the draft stack under the
control directory's salvage area (the same verified, read-back
discipline as git bundles; nothing is pushed unless the remote is
allow-listed), and salvage failure refuses removal. Clean
checkouts at the recorded revision skip salvage. Validate-type
Eden slots, if ever allowed, never salvage, matching git.

### Census and holds

`status`, `doctor` and `audit` treat an Eden row as healthy only
when `eden info` answers for its mount, the state dir is inside
the recorded subvolume, and redirection targets resolve inside
the slot. `audit` reports orphaned-redirection bytes from
`eden du --json` (never by walking the mount) and counts Eden
slots in the leak signal. Holds, heartbeats, handoff, release and
the removal preconditions are unchanged; the census gains the
mount and target paths as additional use evidence, so a running
build under buck-out blocks removal like any other use.

### Measurement plan

On the Phase 2 trial slot, capture before clone, after clone,
after a representative build, and after remove: `btrfs filesystem
usage` for /data and for the image (Metadata Used and allocated,
Data Used), `btrfs qgroup show` for the image, `eden du --json`
and `eden stats` for the trial checkout, and daemon memory. The
question is answered by the delta in /data Metadata Used versus
image Metadata Used for the same work, reported with allocated
alongside, since freed metadata is not free space. A control run
of the same build in a checkout outside the image gives the
comparison arm if the owner wants one.

## Phase 2 entry conditions

Coordinator approval, a slot the owner frees (the host is at the
cap of 3 today, so one existing checkout must be retired by its
owner first, never by this work), and answers for the buck-out
target mechanism and FUSE-on-subvolume behaviour from a first
trial clone. No existing checkout is touched by this design.

## Owner input 2026-11-10 (reply 297): cap and Buck pool

This section builds on the 2026-11-09 Eden concurrency report at
the project root (cited below as the concurrency report); it does
not redo that research.

### Cap: configurable, aiming for about 10

The cap is not fixed at 3. Once Eden metadata is isolated in the
image, the ceiling comes from measured limits: host memory, host
unallocated space, Eden's documented per-host clone limit, and the
eden guard. The concurrency report locks Eden's documented
configured cap at 10 per host (error "maximum number of Eden clones
(10) reached", freed by `eden rm` because the cap counts configured
checkouts) and the guard at WARN 75% / ALERT 85% of allocated
metadata. Design change: the host cap becomes a wrkslots config
value (`eden_host_cap`, default 10 once in-image slots exist),
counted host-wide exactly as designed above; 3 remains only the
transitional owner setting until the trial proves isolation. The
report's older owner count gate of 1 is superseded by this owner
input and must be reconciled by the coordinator, not coded around.

### Buck2 daemon: pooled by concurrency, never shared across roots

Evidence (concurrency report Q4, from live daemon state and the
inspected binary surface): the daemon is keyed by (project root,
isolation dir); no multi-root flag exists, so one daemon cannot
serve several checkouts and re-pointing means starting the target
root's own daemon. Outputs live under `buck-out/<isolation-dir>/`
per root; RE, the remote cache and the host cache daemon are
shared server/host-side, the daemon and `buck-out` are not.
Measured memory is workload-dependent, not a constant: 0.4 GB vs
14.7 GB PSS for two daemons on one root. Idle-timeout and paging
knobs exist in live state but are unset, defaults unresolved.

Pool policy (recommended): N=10 checkouts, M concurrent daemons
bounded by RAM, not by N: at most 2 daemons by default
(`eden_max_daemons`), each slot builds with its own recorded
isolation dir and its buck-out inside the slot. Daemons start on
first build and are killed after the build (`buck2 kill` for that
root only, never `killall`) or after a short idle timeout once its
default is measured; the cost is cold DICE/graph state on the next
build, bounded by the shared RE cache, versus GBs of idle RSS per
retained daemon. Control over buck-out location is exactly:
isolation dir choice, plus the Eden redirection target (scratch
today, slot subvolume under this design) — there is no other
supported relocation on the inspected surface.

### Sized measurement plan (needs coordinator OK; nothing ran)

On the freed trial slot only: cold vs warm daemon start wall time,
RSS/PSS idle, during and after one representative build, kill-after
cost (next-build wall delta), and buck-out growth inside vs outside
the image. Abort on guard WARN or loss of the mount. Numbers from
this plan set `eden_max_daemons` and the idle timeout; until then
both stay at the conservative defaults above.

## Sized trial plan (for coordinator OK; nothing runs before it)

Precondition: the `create --eden` code has landed. Trial slot name
`edentrial` (validate type, never salvaged, retired after). Host
starts at 2 Eden checkouts, so the trial is the third and only new
one; cap config stays at its transitional value for the trial.

Commands (each read-only meter named with its instant):
1. Baseline: `btrfs filesystem usage /data`, `btrfs filesystem
   usage <image mount>`, `btrfs qgroup show <image mount>`,
   `eden list`, `eden du --json <each existing mount>`,
   `eden stats`.
2. `wrkslots create edentrial --slot-type validate --eden
   main=<backing repo>` (creates the subvolume, clones with a
   per-slot config dir inside it, records the row).
3. Post-clone meters as in 1, plus `eden info <trial mount>`,
   `eden redirect list --mount <trial mount> --json`, mountinfo
   for the trial paths, daemon RSS/PSS from /proc.
4. One representative build in the trial checkout; meters during
   and after; daemon start wall time (cold), second build (warm),
   then per-root `buck2 kill` and a third build for the cold-restart
   delta.
5. `wrkslots remove edentrial` after the handoff; post-remove
   meters as in 1; verify the mount is unlisted, targets are gone,
   and orphaned bytes did not grow.

Expected cost: one checkout overlay (order 15 GB materialized from
survey, inside the 256 GiB qgroup), one extra daemon (0.4–15 GB
RSS by workload; abort if host MemAvailable falls below 100 GB),
buck-out inside the slot only. No other checkout is touched.

Cleanup and rollback: any failure before registration removes the
trial subvolume only; after registration, `wrkslots remove` with
the verified orphan checks, then `eden du --clean-orphaned` is
reported, never run silently. Abort lines: guard WARN (75% of
allocated metadata), host unallocated below 150 GiB, or any
unverifiable mount state. Rollback is removal of the trial slot
alone; the two pre-existing checkouts and the shared backing repo
are never modified.

Amendments (coordinator, 2026-11-10): the build target is named
before step 1 from inside the trial checkout by these criteria —
a single modest library target whose cold build is a few minutes,
never a large application target; the exact target string is
recorded here and reported before the build runs. Before step 1,
verify a per-config-dir daemon needs no privhelper or sudo beyond
what the shared daemon already uses; if it does, stop and report.
Run only while the gate queue is not in an owner exclusive phase,
watch MemAvailable throughout (abort below 100 GB), and report the
before/after table (btrfs metadata on /data and in the image,
qgroup, daemon RSS/PSS, cold/warm/cold-after-kill times) into this
issue and to the coordinator, announcing the start of step 1.

### Memory-budgeted pool (owner reply 302)

The owner has seen daemons over 50 GB each, so the measured
0.4–14.7 GB range is not a ceiling and daemon count is the wrong
budget unit. The pool budgets by memory: each Eden slot daemon
runs under a per-daemon cgroup cap (wrkslots box limits:
memory.high for throttling, memory.max as the hard cap; starting
values 32 GiB high / 48 GiB max until measurements set them, so a
runaway daemon is throttled then killed alone, never the host).
A host-wide daemon budget (starting value 128 GiB of summed PSS,
config `eden_daemon_budget_bytes`) is enforced by wrkslots audit:
over budget, the longest-idle daemon is killed first (per-root
kill only). Kill-after-build remains the default; the idle
timeout from the measurement plan applies only under budget.
What drove past 50 GB stays open: wide queries, long-lived growth
and leaks are all plausible; the archived audits read for this
design name no single driver, so the cap treats every daemon as
able to grow until measured otherwise.

### Workload (a) results (2026-11-10, mu-eden-measure isolation)

Target: the modest CLI target named by the coordinator. Three
repeats of fresh-daemon cold then warm, own isolation dir only,
MemAvailable never below about 728 GB (abort line 100 GB).
First-ever cold (daemon start + graph + build): 621 s. Repeat
cold (fresh daemon, warm remote cache and buck-out): 23 s, 21 s
(median 23 s). Warm no-change: 0 s each. Idle daemon after warm:
RSS 3.99 GB, PSS 3.90 GB (one sample, 54 s uptime). Reading: for
this modest target the restart penalty after a kill is about
23 s, so kill-after-build is cheap; holding 3.9 GB idle only pays
for back-to-back builds. The script's first sampler missed the
daemon (its cmdline names neither checkout nor isolation); it now
reads the pid from the daemon info file. Edit-phase and peak-
during-build numbers were not captured and stay open.

Corrected rerun (fixed sampler, all values non-zero): cold wall
19/17/27 s (median 19 s), cold peak RSS about 4.9 GB, PSS about
4.8 GB; warm wall 2-3 s with peak at idle level (idle PSS
4.2-4.6 GB). Edit-phase proxy (own outputs cleaned, rebuild on a
restarted daemon, checkout sources untouched): wall 42 s, peak
RSS 6.24 GB, PSS 6.15 GB, idle after 5.28 GB PSS. MemAvailable
stayed above 728 GB throughout. Own daemon killed after.
