# One btrfs image for a project's whole worktrees tree (design, 2026-11-05)

Status: design + throwaway trial done on devbig014. No live harness was migrated.
Trial (b), migrating this harness's own `worktrees/`, waits for an explicit
coordinator GO. Slug: `wtimg-worktrees-image`.

## 0. What was asked, and what already exists

The owner asked (2026-11-05 06:03/06:34 ET) where the plan went to move a
project's worktrees onto a separate sparse image, corrected to: **one sparse
btrfs image per project holding the project's whole worktrees directory** —
one loop mount, every slot inside it — giving reflinks between slots,
snapshots, zstd and quotas inside the image, and isolation of the host btrfs
metadata. Background on devbig014: `/data` is btrfs (zstd:3) with DUP
metadata, 136 GiB of metadata used, and millions of small files from slots
(`hosts/devbig014/ops/disk-report-2026-11-03.md`).

Search result (step 1, reported to the coordinator before this doc): **no
earlier written plan for this whole-worktrees image exists** in agent-utils
`ai_docs/`, `.minibeads/`, git history (`--grep`/`-S` for btrfs/sparse/loop),
the harness repo (`ai_docs/`, `hosts/*`, ops logs), `~/agent_scratch`, or
dev-hermit's `ai_docs`. What exists is the different, per-slot design the
owner said is *not* this: `ai_docs/transient/2026-09-28-slot-images/README.md`
— one sparse **ext4** image **per slot** (`py/wrkslots/slotimage.py`,
`imagecmd.py`; landed as `cc931d32 wrkslots: disk-image slots and slot
sandboxes`). This design reuses its mount-safety machinery where it fits
(loop-identity checks, mount tables, `sudo -n` helper discipline) but the
unit is the project, not the slot, and the filesystem is btrfs, not ext4.

## 1. Layout and configuration

New opt-in key in `.wrkslots.yml`. Absent means today's behaviour, unchanged.

```yaml
worktrees_image:
  path: worktrees.img        # project-relative backing file, on the host fs
  size_bytes: 1099511627776  # apparent (sparse) ceiling, 1 TiB here
  fs: btrfs                  # only btrfs in this design
  mount: kernel              # kernel only; see §3 for why there is no fuse
```

The image is mounted at the project's slots directory (`worktrees_dir`,
e.g. `worktrees/slots`). Inside the image, each slot may be a btrfs
subvolume (§7). The backing file lives **outside** the mount, beside the
project (never inside its own mount). `path` must be project-relative,
contain no `..`, and not resolve inside `worktrees_dir`.

Why the mount is the slots directory and not literally `worktrees/`:
in flat layout the control directory is `worktrees/` itself (it holds
`ACTIVE.<machine>.json`, journals, `EVENTS.*`, `slot-images/`), and §6
recommends control stay on the host. Mounting the image at `worktrees/`
would put the registry inside the thing it must diagnose. For nested
layout, where control *is* the worktrees directory, enabling
`worktrees_image` requires first splitting control out (a layout
migration), or the feature refuses with that remedy. The owner's "whole
worktrees directory" is satisfied in substance: every slot, every build
tree, every cache — all guest metadata — lives in the one image; only
the small registry files stay outside.

## 2. Finding and verifying the mount (never write underneath)

Every wrkslots command already loads configuration. With
`worktrees_image` configured, loading additionally classifies the mount:

| State | Evidence | Read-only cmds | Mutations |
|---|---|---|---|
| `mounted` | `statfs` on slots dir is btrfs **and** its loop device's backing file is the configured image (device+inode) | proceed | proceed |
| `not-mounted` | slots dir exists, is empty, is not a mount point | report `worktrees-image-not-mounted` + remedy `wrkslots worktrees-image mount` | **refuse** |
| `wrong-mount` | slots dir is a mount, but fstype is not btrfs or the backing file differs | report | **refuse** |
| `not-empty-unmounted` | slots dir is not a mount and is non-empty | report (possible host-underneath slots) | **refuse** |

The hard rule: **wrkslots never creates a slot in an unmounted slots
directory.** An empty directory at the mount point is the classic
loop-mount failure (image not mounted, slots silently created on the
host underneath, then hidden when the image mounts over them). Reusing
the per-slot precedent (`image-not-mounted` in `doctor`/`status`),
`status`/`doctor` report the state and authorise nothing; mutations
refuse. Verification reads `/proc/self/mountinfo` plus the loop
device's backing identity — the same evidence `slotimage.py` already
collects (`LoopEvidence`, `LOOP_GET_STATUS64`) — so a mount copied into
another mount namespace is recognised as the project's own storage,
and an image attached to two loop devices (`attached-twice`) refuses
mount/grow/unmount until a coordinator resolves it.

## 3. Mount lifecycle

- **Who mounts:** the first wrkslots command that needs slots mounted
  does it (idempotent, as per-slot images already remount on every
  config load), plus an explicit `wrkslots worktrees-image mount`.
  Boot mounting is optional: a systemd user unit or an `@reboot` line
  may run the same command, but wrkslots does not require it — first
  use mounts. Mounting while already mounted is a no-op success.
- **Kernel backend (the only one):** `sudo -n mount -o
  loop,compress=zstd:3,noatime <image> <slots-dir>`. `sudo -n` is
  already the wrkslots discipline for loop mounts. The loop device is
  attached with autoclear so unmount frees it (trial verified: after
  `umount`, `losetup -j <image>` is empty — no leaked device).
- **Unprivileged fallback — stated honestly:** there is none for btrfs.
  ext4 has `fuse2fs`; **btrfs has no maintained FUSE driver** (no
  fuse2fs equivalent exists to fall back to). A host without
  passwordless `sudo -n` for mount/loop cannot use `worktrees_image`
  at all. The fallback on such a host is to not enable the feature:
  plain directories, or the existing per-slot ext4 images with their
  fuse backend. The config loader refuses `mount: fuse` for btrfs with
  exactly that explanation rather than pretending parity.
- **Unmount:** `wrkslots worktrees-image unmount` proves no process
  uses any slot (the existing process-use census), syncs, unmounts.
  The next command remounts.

## 4. Growth and shrink/reclaim

- **Grow (online, measured):** `truncate -s <new> <image>` on the host,
  then refresh the loop device's capacity, then `btrfs filesystem
  resize max <mount>`. The middle step is not optional: in the trial,
  truncate 20 GiB→30 GiB + remount + `resize max` silently stayed at
  20 GiB because the loop device kept its attach-time size; only after
  `losetup -c /dev/loopN` did resize report 30 GiB. `worktrees-image
  grow --to <size>` performs all three and verifies `statfs` grew.
  Shrinking the guest (`resize -<n>`) is supported but shrinking the
  sparse file afterwards is not offered; freed blocks return via trim.
- **Reclaim (measured):** the backing file is sparse and mounted with
  discard, and `wrkslots worktrees-image trim` runs `fstrim`. Trial:
  deleting a 50 MiB incompressible file + `fstrim` (29.7 GiB trimmed)
  shrank the host file from 73 MiB to 12 MiB allocated.
- **Backing file on a btrfs host — `+C` ordering (measured):** set
  `chattr +C` (NOCOW) on the **zero-byte** file, *then* `truncate` to
  size, *then* `mkfs.btrfs`. Trial proved the order matters:
  `truncate` first and `chattr +C` afterwards silently leaves the flag
  unset (`lsattr` shows no `C`), so the host would COW-fragment the
  image one extent per guest write. Creation refuses if `+C` cannot be
  verified on a btrfs host. NOCOW on the backing file does not affect
  the guest: guest files still COW, reflink and snapshot normally.
- **Guest profiles:** `mkfs.btrfs -m single -d single`. Inside the
  image, DUP metadata would double every guest metadata write to buy
  redundancy the host md RAID and the image's own scrub story already
  provide; SINGLE keeps guest metadata at one copy (trial guest used
  7.8 MiB total for 20,000 small files). This is the honest trade: a
  single guest metadata block loss is not self-healing inside the
  image; `btrfs scrub` detects, backups/snapshots (§7) recover.

## 5. Crash recovery, fsck and scrub

- The guest is a normal btrfs: an unclean unmount replays on next
  mount. Trial: `kill -9` of a writer mid-write, then unmount/remount
  — files and the wrkslots registry inside the trial project were
  intact and `wrkslots status` reported CONSISTENT.
- wrkslots journals behave as today: trial `SIGKILL` mid-`create`
  left `CREATE.*.journal`; `wrkslots recover` resumed it and
  registered the slot, returning the registry to CONSISTENT (§9).
- `wrkslots worktrees-image status` runs an online `btrfs scrub`
  check on demand (`scrub start -B`; trial: 511 MiB scrubbed, no
  errors) and reports qgroup and usage figures. Full `btrfs check
  --readonly` is an offline, unmounted operation for suspected
  corruption only; it is never run against a mounted image.
- A guest that will not mount: wrkslots reports `wrong-mount`/
  `not-mounted`, refuses mutations, and prints the manual recovery
  commands (`mount -o recovery,ro` / `btrfs rescue`). It never
  auto-repairs and never falls back to using the host directory.

## 6. Control files: outside the image (recommendation)

**Recommendation: the registry, journals, event log and holds stay on
the host filesystem, outside the image.** Only slot directories (and
their build trees/caches) live inside.

Why: (1) Diagnosis — when the image is unmounted, corrupt, or attached
twice, the operator's first tool is `wrkslots status` reading the
registry; a registry inside an unmountable image cannot explain itself.
(2) Durability independence — a guest fs failure must not take the
ownership/handoff record with it; salvage decisions are made from the
registry while the guest may be read-only or gone. (3) Size — control
files are kilobytes; the metadata-isolation win comes from slot
content, not from moving `ACTIVE.*.json`. The cost is one split to
explain (slots in, registry out), which §2's mount check makes safe:
mutations refuse unless the slots mount verifies, so the outside
registry can never trick wrkslots into writing slots onto the host.
Per-slot image records (`slot-images/`) likewise stay outside, beside
the registry, as today. The trial project (§9) deliberately kept its
control inside to prove the lifecycle works either way; the shipped
layout keeps it outside.

## 7. Quotas, snapshots, reflink clones

- **Per-slot subvolumes + qgroups:** with the feature on, `create`
  makes each slot directory a btrfs subvolume inside the image and
  `btrfs quota enable` runs once at image creation. An optional
  `slot_quota_bytes` then maps to `btrfs qgroup limit`. Trial: a
  50 MiB limit bit exactly (write stopped at 49.98 MiB, `EDQUOT`).
  Honest caveat, measured: qgroups count *referenced* (post-
  compression) bytes — 80 MiB of compressible data referenced only
  1.61 MiB and did not bite. Quotas bound disk cost, not apparent
  bytes; document them that way. Qgroup accounting costs some guest
  write overhead and large deletions can trigger rescans inside the
  guest (contained: the host fs pays nothing).
- **Snapshots:** `btrfs subvolume snapshot` of a slot is near-instant
  (trial: 0.046 s for 20,001 files). Uses: pre-`convert`/pre-remove
  safety copies and cheap slot templates. Snapshots are not backups
  (same image file); they protect against operator error, not media
  loss. Snapshot retention needs a policy (name, count, who deletes)
  before it is automated; this design only exposes the primitive.
- **Reflink clones for fast slot creation:** files cloned between
  slots inside one image share extents (`cp --reflink`). This finally
  allows what per-slot ext4 images cannot: a donor build cache or a
  template checkout cloned into a new slot at metadata cost. It does
  not change lifecycle policy — a cloned cache is still a configured
  cache glob, reclaimable as today. `git worktree` creation itself is
  already cheap; reflink pays for caches and vendored trees.

## 8. Migration of an existing project, and rollback

Preconditions: slots idle or quiesced (no slot in use; `wrkslots
audit` clean or every exception understood), validation green, and
free host space ≥ the slots' current allocated bytes.

1. Write the rollback plan (below) before touching anything.
2. `wrkslots worktrees-image create --size <n>`: zero-byte file,
   `+C` verified, truncate, `mkfs.btrfs -m single -d single`,
   quota enable. Image stays unmounted; nothing has changed yet.
3. Rename the current slots directory aside (`slots` → `slots.host`)
   and create the empty mount point at the *same path* (absolute
   paths in Git worktree metadata require the same path, §10).
4. Mount the image at that path; create per-slot subvolumes.
5. Copy with reflink where possible (`cp -a --reflink=always` from
   `slots.host`, or `rsync` for the first copy), preserving modes,
   times and xattrs; compare full listings (path, type, size, mode,
   mtime) exactly as `image convert` already does per slot.
6. Verify: `git worktree list --porcelain` in each source repo,
   `wrkslots status` CONSISTENT, one real build/test in one slot.
7. Add `worktrees_image:` to `.wrkslots.yml` (this arms §2's guard).
   Only now do new commands require the mount.
8. Keep `slots.host` untouched for the agreed soak (days), then
   reclaim it (`clean-caches` first, then remove) as a separate,
   explicitly approved step.

**Rollback** (any time before step 8's reclaim): stop slot processes,
`wrkslots worktrees-image unmount` (or `sudo umount`), remove the
empty mount point, rename `slots.host` back to the same path, remove
the `worktrees_image:` config block, verify `wrkslots status`. The
image file may be kept for a retry or deleted; no slot data was only
in the image unless work continued after step 6 — so rollback after
real work means copying *back* first (steps 5–6 reversed), never just
renaming. That asymmetry is why migration happens quiesced.

## 9. Trial (a) results — throwaway, devbig014, 2026-11-05

Project: `/home/newton/temp/wtimg-trial/` (`host/worktrees.img`,
20 GiB sparse, mounted at `mnt/`; left unmounted, available for
inspection). Host `/data`: btrfs zstd:3, 649 GiB available.

| Check | Result |
|---|---|
| Create image | `mkfs.btrfs -m single -d single` in 0.086 s; host allocated 4.3 MiB for a 20 GiB sparse file |
| `+C` ordering | `truncate`→`chattr +C` silently unset; 0-byte→`+C`→`truncate` verified set |
| 20,000 × 100 B files | guest 0.51 s vs host 0.66 s; guest total used 7.8 MiB; host file 4.7 MiB while the same host-dir probe showed 79 MiB apparent — guest metadata stays in the image |
| Snapshot | subvolume snapshot of 20,001 files in 0.046 s |
| Reflink | `cp --reflink` of a 100 MiB file instant (compressible-data caveat below) |
| Quota | 50 MiB qgroup limit: compressible writes did **not** bite (1.61 MiB referenced); `urandom` bit at 49.98 MiB with `EDQUOT` (errno 122) |
| Grow | 20→30 GiB: stayed 20 GiB until `losetup -c` refreshed the loop device, then `resize max` → 30 GiB online |
| Reclaim | delete 50 MiB + `fstrim` (29.7 GiB trimmed): host file 73 MiB → 12 MiB |
| Remount | unmount/remount: data persistent, `losetup -j` empty after unmount (no leaked loop device) |
| wrkslots lifecycle | `init` + `create s1` (0.89 s, plain-worktree representation inside the guest) + commit: all normal; `status` CONSISTENT |
| Crash: killed create | `SIGKILL` mid-`create` left `CREATE.*.journal`; `wrkslots recover` resumed and registered slot `s2`, back to CONSISTENT |
| Crash: killed writer | `kill -9` mid-write + remount: files intact, registry CONSISTENT; online scrub (511 MiB) no errors; offline `btrfs check --readonly` clean |
| finish/remove | **not exercised**: trial owners were short-lived shell PIDs, so `finish` correctly refused (`owner PID is not live`). Removal additionally needs the recorded handoff + heartbeat TTL (3600 s). These paths are representation-independent and covered by the existing wrkslots e2e; a trial with a long-lived owner is the follow-up if wanted. |

Compressible-data caveat applies to two rows: with `compress=zstd:3`,
quotas and `du` see compressed bytes. All quota/size claims above use
incompressible data where it matters.

## 10. Risks

- **Loop device leak.** A crashed mount can leave a device bound.
  Mitigation: autoclear on attach, `worktrees-image status` lists
  devices holding the image (`losetup -j` + backing identity),
  unmount verifies the device is freed. Trial: none leaked.
- **Image file on a btrfs host.** Without NOCOW the host fragments
  the image per guest write (§4 ordering, verified at create). Host
  compression of the backing file is also wasteful double compression
  (guest already zstd) — another reason `+C` (which disables host
  compression for the file) is required, not optional.
- **Metadata profile inside the image.** SINGLE (§4) trades guest
  self-healing for half the metadata writes; DUP inside a file on a
  host that already survived on DUP is defensible but doubles the
  exact cost this feature exists to contain. Revisit if scrub ever
  reports unrecoverable guest metadata errors in practice.
- **Git worktree absolute paths.** `.git` files and the Git common
  dir record absolute slot paths. The image must always mount at the
  same path (it does — the path is the project's slots directory,
  independent of where the backing file lives). Moving the *project*
  breaks worktrees with or without this feature and needs the usual
  repair/`relocate-repository` path; moving the backing file does not.
- **One image is one failure domain.** Guest corruption or an
  accidental `rm` of the backing file takes every slot at once,
  where plain directories fail one at a time. Mitigations: registry
  outside (§6), snapshots for operator error, and the project's
  existing salvage (work is pushed/landed, slots are reconstructible).
  The image file must be excluded from ad-hoc cleanup globs.
- **ENOSPC moves, it does not vanish.** A runaway slot now fills the
  image (all slots) instead of the host (everything). Per-slot
  qgroups (§7) plus image-level free-space alarms are the control;
  sizing guidance: apparent size is free (sparse), so size generously
  and alarm on host-allocated bytes.
- **Double filesystem cost.** Every guest write pays guest btrfs +
  host btrfs. Small-file latency in the trial was comparable
  (0.51 s vs 0.66 s), but fsync-heavy loads should be re-measured
  before trial (b); per-slot ext4 measured FUSE at ~10× slower for
  metadata, kernel loop was near-native, and this design is
  kernel-only for the same reason.

## 11. Implementation plan (opt-in, default unchanged)

1. Config: parse/validate `worktrees_image` in both editions
   (Python `cli.py`; Rust `wrkslotsd` config validation mirrors it),
   `fs: btrfs` and kernel mount only, fuse refused with §3's reason.
2. `py/wrkslots/worktreesimage.py`: create (with `+C` ordering),
   mount-state classification (§2 table), mount/unmount/grow/trim,
   reusing `slotimage.py`'s mount-table and loop-identity helpers.
3. Guard: config load classifies the mount; mutations refuse on
   `not-mounted`/`wrong-mount`/`not-empty-unmounted`; `status`/
   `doctor` report `worktrees-image-*` findings and authorise nothing.
4. `wrkslots worktrees-image create|mount|unmount|status|grow|trim`.
   Per-slot subvolume creation in `create` behind the feature flag.
5. Tests: `py/wrkslots/tests/test_worktrees_image.py` — config
   validation, mount classification against fake mount tables
   (including the unmounted-empty-mountpoint refusal), `+C` ordering,
   and a sudo-gated real lifecycle mirroring §9. Default-off covered
   by asserting today's behaviour without the key.
6. Docs: `common/docs/wrkslots/USER_GUIDE.md` section + the skill
   procedure (§12 of the landing change), each stating the kernel-only
   requirement where an operator will see it before enabling.

## 12. Revision 2 (2026-11-05, owner decision 08:13 ET)

The owner adopted this as the direction for **all** worktrees: a
first-class wrkslots storage mode beside host-disk and per-slot-ext4,
with built-in live migration and a per-slot migrate command. The
per-slot ext4 mode was rejected for large repositories because it gives
up sharing and copy-on-write between slots; the project image keeps
both (reflink/snapshot inside one btrfs). The owner's "two-level
image" idea is noted only, not a feature request.

Implemented as step 2 on top of step 1 (landed `194e2814`):

- **A. First-class mode.** With `worktrees_image` mounted, `create`
  makes each new slot a btrfs **subvolume** in the image;
  `slot_quota_bytes` in the section sets its qgroup limit. Decision:
  subvolume-per-slot + qgroups (the owner's isolation-plus-sharing
  question) — sharing via reflink/snapshot inside the image, bounds
  via qgroups, at the measured cost that quotas count referenced
  (post-compression) bytes. Emptied slot subvolumes are deleted with
  `btrfs subvolume delete` in the normal removal path.
- **B. Live migration.** `wrkslots migrate <slot>` copies one idle
  slot into a subvolume at the *same path*, verifies the listing file
  by file, swaps, and deletes the original; the registry record never
  changes. Project-level cutover follows the owner's sequence —
  rename `worktrees` → `worktrees_old`, mount the image at
  `worktrees`, move slots one at a time, delete the old tree last —
  rehearsed below. Journal/registry implication, measured: at
  cutover there must be **no outstanding journals** (migrate/create
  refuse otherwise in spirit; the operator runs `wrkslots status`
  first), and because slot paths do not change, no registry rewrite
  is needed; events gain only the normal lifecycle entries.
- **C.** is the subvolume/qgroup decision above.

### Measurements (owner questions 1–4), throwaway `wtimg-m2`/`wtimg-m4`

| Question | Result |
|---|---|
| (1) delete → host return | With `discard=async` (the host's default), deleting 512 MiB in the guest returned host allocation 517 MiB → 5 MiB within 15 s of `sync`, no pause and no explicit trim; a later `fstrim` reclaimed only residue (5 MiB → 2 MiB). No build-latency impact was observed during the churn writes; trim is a backstop (e.g. after bulk slot removal), not the mechanism. |
| (2) host ENOSPC | On a size-capped tmpfs host driven to 100%, the kernel logged `BTRFS warning: unexpected block io error` but the guest stayed read-write, scrub found no errors, and all data read back after freeing host space. Read-only abort was *not* reproduced — tmpfs page-cache buffering absorbs what a disk-backed host would surface as EIO at writeback, so the honest claim is narrower: protect with headroom monitoring plus a partial `fallocate` floor (preallocate a floor, e.g. 10% of apparent size, so guest metadata always has backing) and alarm on host-allocated bytes. Never let the host reach 0 under a sparse image. |
| (3) fragmentation | Identical 512 MiB + 2,000 random-overwrite churn: backing file with `+C` had **11 extents**, without `+C` **54 extents**. NOCOW measurably contains host fragmentation; the gap grows with churn age. |
| (4) in-flight migration | Rehearsed end to end: rename, mount at the same path, per-slot subvolume + copy, status CONSISTENT, Git history intact. Two findings: `cp --reflink=always` across host→image fails `EXDEV` (different filesystems) — migration copies are plain, reflink pays only *inside* the image; and a partial copy leaves a broken `.git` gitfile, so verification (full listing + Git) must precede the swap, as `wrkslots migrate` does. In flat layout, mounting at `worktrees/` also moves the control directory inside the image; §6's recommendation (mount at the slots directory, control outside) stands unless the operator accepts the copied-registry cutover. |

## 13. Open questions

- Snapshot retention policy (count/age/who approves deletion).
- Default `size_bytes` and the host-allocated alarm threshold.
- Whether `create` should default new slots to subvolumes immediately
  or only when `slot_quota_bytes` is set (subvolume deletion needs
  `btrfs subvolume delete`, which changes `remove`'s rmdir path).
- Trial (b) window and soak length for this harness's own migration.
