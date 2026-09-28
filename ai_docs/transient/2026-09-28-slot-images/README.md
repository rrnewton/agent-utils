# Slot images and slot sandboxes (proof of concept, 2026-09-28)

Status: working design with a working implementation on branch `slot-images`. Settled
user-facing text lives in `common/docs/wrkslots/USER_GUIDE.md` ("Disk-image slots" and
"Running commands and agents inside a slot's box").

## Problem

Agents working in long-lived slots on one large development host write enormous numbers of small
files (checkouts, build trees, caches) straight into the host file system, and a runaway process
can fill the host. Two host failure modes follow:

1. **Bytes.** One process writes until the device is full.
2. **Metadata.** On a copy-on-write file system with a doubled metadata profile, millions of
   small files consume metadata block groups. Once the device has no unallocated space left for a
   new metadata chunk, the file system aborts transactions and turns read-only, while `df` still
   shows hundreds of GB free inside data chunks.

A per-run btrfs subvolume with a qgroup limit bounds bytes, but it requires quotas on the whole
host file system (every write on the host then pays qgroup accounting, and large deletions
trigger rescans) and root to delete each subvolume; a caller that loses privilege leaks one
subvolume per run.

Constraints from the owner:

- No persistent host changes: no repartitioning, no mount options, no enabling quotas, nothing a
  configuration manager would revert or a fresh host would lack. A short-lived `sudo -n` helper is
  acceptable; an unprivileged fallback is wanted.
- No pre-reservation: a slot has no natural "full size".
- Representation must not change lifecycle policy (ownership, salvage, reclaim).
- Existing slots must keep working when the default changes, with an in-place migration.
- Builds should keep final outputs in the slot but leave intermediates in an action cache outside
  it.

## Design

### Storage: one sparse ext4 image per slot

`<control>/slot-images/<type>/<slot>/{IMAGE.json, slot.img, state.img, state/}`. `slot.img` is
mounted at the slot directory; `state.img` holds the sandbox's private `$HOME` state and `/tmp`.

- Images are created no-copy-on-write (`chattr +C` on the empty file) so a copy-on-write host
  stores each image as a few large extents, not one extent per guest write, and are sparse:
  `ceiling_bytes` bounds the apparent size and reserves nothing.
- `mkfs.ext4 -m 0 -J size=64 -E root_owner=UID:GID,nodiscard,lazy_itable_init=1`. A fresh image
  costs about 70 MiB of host space.
- Kernel mounts use `-o loop,discard`, so deletions punch holes in the backing file. Measured on a
  100 GiB image: 518 MiB allocated after mkfs (default journal), 2.6 GiB after writing 2 GiB,
  530 MiB after deleting it. FUSE mounts return space on `fstrim` (measured 1.1 GiB back to
  531 MiB).
- Backends: `kernel` (`sudo -n mount -o loop`) and `fuse` (`fuse2fs`, run in a transient user
  service so it outlives the command that mounted it). Creating 50,000 empty files took 1.47 s on
  the kernel mount; 5,000 took 1.63 s on FUSE, so FUSE is roughly ten times slower for
  metadata-heavy work.
- Every configuration load remounts image-backed slots that are not mounted, at the location
  recorded in `IMAGE.json`.

Lifecycle integration points in `cli.py`, all behind helpers that fall back to the old behavior
for plain slots:

| Point | Plain slot | Image slot |
|---|---|---|
| create | `mkdir` (nested layout) | provision images, mount at the slot path |
| path fence and its rollbacks (10 rename sites) | `os.rename` | unmount, rename the empty mount point, remount |
| remove fenced/aborted slot directory | `rmdir` | refuse unless only mkfs residue remains, then destroy the images |
| flat-layout `git worktree remove` | normal | accept Git's expected EBUSY on the image root after it deleted the content and the administrative entry |
| process-use census | any mount inside the slot is use | the slot's own image mount lines are excluded |

Representation is per slot: a slot is image-backed exactly when its `IMAGE.json` exists. Nothing
was added to the registry records, so the replay schema is unchanged.

Found and fixed during testing: after `fusermount -u`, the kernel detaches the mount before
`fuse2fs` has written its cached metadata. Remounting immediately started a second server that
read stale metadata, and a file deleted just before the path fence reappeared, which the salvage
check then correctly reported as "changed after salvage". Unmount now waits for the old server
to exit, and mount refuses while one is still running.

### Migration

`wrkslots image set-default image|worktree` changes only new slots. `wrkslots image convert SLOT
--to image|worktree` copies an idle slot (no process using its path), compares full listings
(path, type, size, mode, mtime), swaps at the same path, and re-verifies Git worktree identity.

### Sandbox: `wrkslots run SLOT -- COMMAND`

- **Limits.** A transient systemd user scope for the calling process itself, in
  `<enclosing>-wrkslots-<slot>.slice` (created through the user manager's D-Bus API with the
  caller's PID, then exec). The slot slice is nested inside whatever slice the caller already
  occupies, so a site policy that confines a caller to its own slice is never escaped.
- **PID stability.** Every step execs; the PID never changes. A terminal multiplexer that only
  starts agents when the pane's own shell PID is the foreground process (Herdr checks exactly
  this) accepts the boxed shell. `wrkslots shell-command` prints such a command line with absolute
  interpreter paths, because a forking interpreter shim anywhere in the chain breaks the check.
- **File-system view.** `unshare(CLONE_NEWUSER | CLONE_NEWNS)`, user mapped to itself, `/`
  made private, then: private `$HOME`, redirects (`/tmp`, `home_writable`), read-only and writable
  binds, then every remaining mount remounted read-only (keeping each mount's locked
  `nosuid/nodev/noexec/atime` flags). Mounts that are shadowed (`EINVAL`) or that the user could
  not write anyway are skipped; any other failure refuses to run.
- **HOME.** An overlay of the real `$HOME` was tried first and is impossible from a user
  namespace when `$HOME` has submounts ("failed to clone lowerpath": the kernel refuses to clone
  a mount with locked children). The implemented design is a private per-slot, per-mode `$HOME`:
  `ro` binds every top-level directory read-only and seeds private copies of top-level files up to
  1 MiB (so `~/.claude.json`-style atomic rewrites work), `select` exposes listed paths, `none`
  nothing. `home_shared` (agent credentials and transcripts) stays shared and writable.
- **Limitation.** Setuid helpers cannot gain privilege inside a user namespace, so `sudo` and any
  harness launcher that enters a site sandbox through a setuid helper fail under
  `--isolation namespace`. `--isolation cgroup` keeps the limits and leaves file-system
  confinement to the harness's own sandbox.

### agentctl

`agentctl start NAME --slot SLOT [--slot-isolation namespace|cgroup] [--slot-project DIR]`
(Python edition): asks `wrkslots shell-command --format json` for the command and slot path,
records the slot path as the agent's cwd, runs the command in the new pane, waits until the pane's
shell PID is again the sole foreground process, is a shell, and sits in a wrkslots slice, then
starts the harness normally. Verified end to end: an interactive harness started through Herdr
in a boxed pane, with its process in `wrkslots-<slot>.slice` and cwd at the slot. The Rust edition
does not implement `--slot` yet.

### Builds

A build tool that materializes outputs on demand keeps only final outputs in the slot. Verified
with Buck2 inside an image-backed slot and the namespace sandbox: with `materializations =
deferred` and remote execution, a genrule producing a 50 MB intermediate and a 9-byte final
output left only the final output in the slot (`buck-out` 26 MB, mostly daemon state; the
intermediate never materialized). A second slot building the same targets reported 100% cache
hits and also did not materialize the intermediate. With local-only execution, the intermediate
is written inside the slot (74 MB `buck-out`). Any remote-execution API cache server outside the
slot works the same way; the sandbox leaves the network alone. `~/.buck` is a private
`home_writable` directory, so each slot has its own daemon state.

## Verification

- `py/wrkslots/tests/test_slot_images.py`: configuration defaults and migration rules, settings
  validation, slice nesting, owned-mount exclusion, a real image lifecycle including the
  delete-then-relocate regression, and a real sandbox run that proves writes outside the slot
  are refused.
- `py/wrkslots/tests/e2e_images.sh BACKEND LAYOUT`: init (image default), create, content, status,
  sandbox runs in two HOME modes, owner death, salvage-and-remove, a plain slot in the same
  project, convert to image and back, remove. Passed for `kernel nested`; see the pull request
  for the other combinations. Most of the 13-minute wall time is the pre-existing process census
  (`lsof +D` and `/proc` walks over thousands of host processes), not the images.

## Open questions

- Full clone per slot (copy-on-write template image with its own Git objects) instead of linked
  worktrees: stronger isolation of Git objects, but it changes the salvage and registration model.
- Host-wide early warning: read the file system's unallocated-space counters and freeze slot
  slices below a threshold.
- The Rust agentctl edition.
