# #1409 eden-fuse-image: an unprivileged per-slot FUSE image path for `create --eden`

Status: design + experiment complete, 2026-11-12 (experiment section
below: a real Eden clone inside a fuse2fs `allow_other` image succeeded
with no sudo). No wrkslots behaviour changes under this issue. The interim behaviour is #1408 eden-image-refuse: `create --eden`
with `--representation image` or an image ceiling flag refuses before anything
is created. This note gives the owner the facts to choose between:

- **Option S (sudo):** allow `sudo -n` for the project-wide worktrees image on
  this host (one loop-mounted btrfs image, each slot a quota'd subvolume), or
- **Option F (this note):** a per-slot fuse2fs image mounted with
  `-o allow_other`, no sudo, so Eden's setuid privhelper can mount inside it.

## Where things stand today (verified in the current checkout)

- `_cmd_create` in `py/wrkslots/cli.py` returns from the Eden branch
  (`_provision_eden_slot`) before the per-slot image branch
  (`_provision_slot_image`). `_provision_eden_slot` only `mkdir`s the slot, or
  creates a subvolume when a project-wide `worktrees_image` is configured.
  #1408 now refuses the per-slot image flags on this path instead of silently
  ignoring them.
- The project-wide model (`py/wrkslots/worktreesimage.py`) is kernel-only by
  design: `sudo -n mount -o loop`, btrfs subvolume create, `chown`. Its module
  docstring states there is no unprivileged fallback for btrfs.
- The per-slot model (`py/wrkslots/slotimage.py`) has two backends. `kernel`
  needs `sudo -n`; `fuse` runs `fuse2fs` as the invoking user and needs no
  privilege. `resolve_backend` picks `kernel` under `auto` whenever
  `sudo_available()` (`sudo -n true`) succeeds, so a no-sudo caller must pin
  `WRKSLOTS_IMAGE_BACKEND=fuse` to keep the backend from even probing sudo.
- `_fuse_mount` in `slotimage.py` starts fuse2fs (as a transient user service
  where `systemd-run` exists) with no `-o allow_other`. FUSE therefore
  restricts the mount to the mounting user. Eden mounts go through the
  setuid-root `edenfs_privhelper`, a different uid, so an Eden mount inside
  such an image is expected to be refused. That inference is what the
  experiment below tests around, without an Eden clone.
- `/etc/fuse.conf` on this host sets `user_allow_other`, so an unprivileged
  user is permitted to pass `-o allow_other` at all.
- Image removal is intentionally strict: `slotimage.destroy` refuses while
  the mounted image root holds anything but mkfs residue (`lost+found`),
  unless a caller that has already salvaged the content passes
  `allow_content=True`. No lifecycle caller passes it today.
- The host Eden checkout cap is full (3 of 3). No experiment here clones an
  Eden checkout.

## Option F design

### Create

1. `create --eden --representation image` stops refusing only when the
   resolved image backend is `fuse`. A `kernel`-resolved image keeps the
   #1408 refusal: the point of this path is that it never needs sudo, and a
   silent fallback to a sudo mount would change the host's privilege surface
   without the owner choosing it. Pinning is explicit, for example a new
   image record field or the existing `WRKSLOTS_IMAGE_BACKEND=fuse` /
   configured `backend: fuse`; the create output names the backend used.
2. Provision the per-slot images exactly as `slotimage.provision` does today
   (sparse `slot.img` mounted at the slot directory, `state.img` for the
   private `$HOME` layer), with one change: the slot image's fuse2fs is
   started with `-o allow_other`. The state image does not need
   `allow_other` for Eden and keeps the default, so the private `$HOME`
   layer is not part of the exposure below.
3. With the slot directory now a mounted image, run the existing Eden
   provisioning inside it: `.eden-state` in the slot, `eden clone` with that
   config dir, the mount at `<slot>/checkout`, and the existing checks that
   every redirection target stays inside the slot (`eden.is_inside`). The
   Eden branch must therefore run after image provisioning, not return
   before it, and a failed clone must leave the image destroyable: unmount
   the checkout if one was made, empty the image, then `destroy`.
4. Record what doctor and remove later need in the places that already hold
   them: the checkout row (`vcs="eden"`, as today) and `IMAGE.json`
   (backend `fuse`). No new registry field is needed to know a slot is an
   Eden-in-FUSE-image slot; that is exactly the two records together.

### Remove

The order is forced by two independent refusals, and both are correct:

1. Salvage first, as for any Eden slot: if Sapling reports uncommitted work
   or the checked-out revision moved (`eden.salvage_needed`), preserve it
   (`eden.salvage_commands`) before anything is removed.
2. `eden remove` the checkout, with the slot's own config dir, and prove it
   (`eden.verify_removed`): the mount is unlisted, every redirection target
   is gone, orphaned-redirection bytes did not grow. While an Eden mount is
   live inside the image, the fuse2fs mount is busy and the image root is
   non-empty, so image destruction cannot and must not proceed.
3. Delete the remaining in-image state (`.eden-state`, any redirection
   targets Eden left) so the image root is empty but for `lost+found`.
4. Only then `slotimage.destroy`, which unmounts fuse2fs and deletes
   `slot.img` / `state.img`. `allow_content=True` is never the Eden path: it
   would delete an image under a checkout that was never proven removed.

If step 2 cannot complete (daemon down, checkout wedged), remove refuses and
leaves slot, image, and checkout untouched. The documented recovery is to
repair or remove the Eden checkout by hand, then rerun remove; there is no
force flag that deletes the image around a live checkout.

### Doctor

Doctor gains checks, all read-only, none needing sudo:

- `IMAGE.json` agrees with the registry: an Eden checkout row on an
  image-backed slot whose backend is not `fuse`, or whose slot image is
  mounted without `allow_other` in the host mount table, is a finding that
  names the fix (recreate the slot; the mount option cannot be changed on a
  live fuse2fs).
- The fuse2fs server for `slot.img` exists and serves that image file
  (the `_fuse_servers` scan), and the mount is at the recorded location.
- `eden list` for the slot's config dir lists `<slot>/checkout`, and the
  host-wide cap accounting still parses; a daemon that cannot be queried is
  reported, never read as "no checkout".
- A missing `allow_other` is reported as "Eden's privhelper cannot mount
  inside this image", not as a generic mount problem.

### What `allow_other` exposes — plainly

This is the cost of Option F, and it is a real weakening of the slot
boundary:

- Without `allow_other`, a FUSE mount is visible only to the user who
  mounted it. With it, **any local user on the host can access the mounted
  slot image**, subject to the file permissions inside the ext4 image.
- Those permissions are the only remaining barrier, and slot content is not
  written defensively: the slot mount point is mode 0755, and checkouts,
  build output, and tool state are typically world-readable. In practice,
  another local user could read the Eden checkout's contents and anything
  else the agent wrote into the slot image. Write access follows the same
  permissions wherever they allow it.
- That exposure is also the mechanism: `edenfs_privhelper` runs as root, a
  different uid from the slot owner, and root is exactly an "other" user to
  FUSE. There is no FUSE option that admits only the privhelper.
- The exposure lasts while the image is mounted, which for a live slot is
  its whole life. The state image (private `$HOME` layer) is a separate
  mount and is not given `allow_other` in this design, so credentials staged
  there are not exposed by this change; anything an agent copies into the
  checkout or slot root is.
- On a single-user host this may be an acceptable trade. On a shared host
  it hands every local account read access to every Eden slot's files.
  Option S (sudo, kernel mounts) has no equivalent exposure: kernel mounts
  follow ordinary permission checks without a FUSE gate to open.

### Failure modes

| Failure | Behaviour |
|---|---|
| fuse2fs dies, image file intact | Slot paths look empty; `ensure_mounted` remounts (with `allow_other` again) before lifecycle work. Nothing was deleted. |
| Host reboots | fuse2fs user services are gone; images are sparse files on disk and remount on demand, as today. Eden daemons also need their normal restart. |
| Eden clone fails mid-create | Create refuses, removes any partial checkout, empties and destroys the image; no registry row is written. A leftover is a `provisioning`-phase image record, which existing recovery already recognises. |
| `eden remove` wedged at slot removal | Remove refuses at the verify step; image and checkout stay. Manual Eden recovery, then rerun. No force-delete. |
| Image fills its ceiling | Writes inside the slot fail with ENOSPC inside the image; the host file system is unaffected. `image grow` raises the ceiling, as today. |
| fuse2fs metadata slowness | fuse2fs is the slower backend for metadata-heavy work (module docstring); an Eden checkout is metadata-heavy, so this path may be noticeably slower than a kernel mount. Unmeasured for Eden. |
| `user_allow_other` removed from `/etc/fuse.conf` | fuse2fs with `-o allow_other` fails to mount; create refuses at provisioning and doctor names the missing option. |
| Another local user mounts or reads the slot | See the exposure section: reads are expected behaviour of the option, not a malfunction. |

## Experiment on this host (no sudo, real Eden clone)

Run 2026-11-12 on devbig030 as this user, after the owner authorised more
Eden checkouts inside an image. Constraints held: no `sudo` in any form
(not even `sudo -n true`); the outer image sparse, 8 GiB ceiling in
`/tmp/mu-edenimg-fuse-exp` (69–73 MiB actually allocated, far under the
10 GiB cap); the host's three existing checkouts were only read
(`eden list`, `eden info`), never modified; everything below was
`eden remove`d, unmounted, and deleted afterwards. Three runs: run 1 with a
plain `mkfs.ext4` failed for an instructive reason (below); runs 2 and 3
with wrkslots' own `root_owner` form succeeded identically.

### What was run

```sh
truncate -s 8G outer.img
mkfs.ext4 -q -E root_owner=$(id -u):$(id -g) outer.img
fuse2fs -o allow_other outer.img mnt &
# nested FUSE inside it:
truncate -s 512M mnt/nested.img; mkfs.ext4 -q -E root_owner=$(id -u):$(id -g) mnt/nested.img
fuse2fs mnt/nested.img mnt/nested &
BACKING=$(eden info <existing-host-checkout>)   # its backing_repo, read-only
eden --config-dir mnt/.eden-state clone "$BACKING" mnt/checkout
sl -R mnt/checkout status
head mnt/checkout/AGENTS.md                 # file content read
echo eden-fuse-probe > mnt/checkout/probe-file.txt; cat mnt/checkout/probe-file.txt
eden --config-dir mnt/.eden-state remove mnt/checkout
eden --config-dir mnt/.eden-state stop; fusermount -u mnt/nested; fusermount -u mnt
```

### Observed

1. **fuse2fs with `allow_other` mounts.** The host mount table records it:
   `fuse.ext4 … user_id=212630,group_id=100,allow_other` at the mount
   point. `/etc/fuse.conf` has `user_allow_other`; `/dev/fuse` is 0666.
2. **A second FUSE filesystem mounts inside it, as this user.** The nested
   image's mount sits under the outer mount in `/proc/self/mountinfo`, and
   files round-tripped through both layers (`hello-outer`, `hello-nested`).
3. **Eden's privhelper mounts inside it. The clone succeeded.**
   `eden clone` printed `Success. Checked out commit 01b726eb` (run 2:
   `ce90aaf9`; the backing moves). The mount table shows the Eden mount
   nested under the fuse2fs mount: `fuse edenfs: …
   default_permissions,allow_other` at `mnt/checkout`, parented on the
   outer fuse mount. This is the cross-uid mount the design exists to
   enable — proven, not inferred.
4. **The daemon serves from inside the image.** Its state dir was
   `mnt/.eden-state` on the image; `eden list` for that config dir listed
   the checkout; `sl -R <checkout> status` exited 0 with no output (clean);
   a file content read returned the checkout's `AGENTS.md` head; a write
   plus read-back inside the checkout round-tripped (`eden-fuse-probe`).
   Actual host allocation for the whole stack: 73 MiB of the 8 GiB image.
5. **`eden remove` unmounts the nested mount cleanly.** It printed
   `Success`, the checkout mount disappeared, and after
   `eden … stop` (`edenfs exited cleanly`) both fuse2fs mounts unmounted
   with `fusermount -u`. Verified at the end: no experiment mount in
   `findmnt`, no fuse2fs/edenfs process for the experiment, the directory
   deleted, `eden list` back to the host's original three checkouts.
   One wrinkle for the remove design: in run 2, `eden stop` immediately
   after `eden remove` reported `edenfs is not running` while the daemon
   (one verified PID, `--edenDir` inside the image) was in fact still
   alive and holding the image busy; it had to be signalled directly.
   Run 3's `eden stop` worked cleanly. Remove must therefore verify the
   daemon is gone — and the image unmountable — rather than trust the
   stop result, which the doctor checks above already require.
6. **Run 1's failure, and why it matters for create:** with a plain
   `mkfs.ext4`, the fresh image's root is owned by root and this user
   could not write at all (`Permission denied` on the first probe file,
   and `eden clone` failed with `failed to write the daemon args file …
   '/…/mnt/.eden-state'`). wrkslots already does this right:
   `make_image_file` in `slotimage.py` passes
   `-E root_owner=<uid>:<gid>` to mkfs. Any Eden-in-image create must keep
   that, or it fails before Eden starts.
7. **A redirection escapes the image today.** The clone's `buck-out`
   redirection bind-mounted host btrfs scratch
   (`/users/newton/scratch/…/edenfsZredirectionsZbuck-out`) into the
   checkout — outside the image and outside the slot path. The existing
   wrkslots guard (`eden.is_inside` on every redirection target, refusing
   in `_provision_eden_slot`) would refuse such a clone. An implementation
   must configure Eden redirections to land inside the slot (or refuse),
   it cannot just clone with defaults.

## What still needs to prove

Proven above: the `allow_other` mechanism, FUSE-in-FUSE as this user, a
real Eden clone with the privhelper mounting inside the image, serving,
reading and writing, and clean `eden remove` plus unmount. Still unproven
for a wrkslots implementation:

1. The full wrkslots remove sequence end to end: salvage, `eden remove`,
   daemon-gone verification (observed wrinkle 5), image-empty proof, and
   `slotimage.destroy` — including stopping the daemon in an order that
   never leaves the image busy.
2. Redirections confined inside the slot (observed escape 7), so the
   existing `eden.is_inside` guard passes without weakening it.
3. Lifecycle survival: fuse2fs server death and remount, daemon restart,
   and host reboot, in an order wrkslots can automate (image mounted
   before the daemon needs it).
4. Performance: checkout, status, and build latency on fuse2fs versus a
   plain-directory Eden slot, enough to judge the slower-backend warning
   in the failure table. (The clone itself took seconds; that is Eden's
   lazy materialisation, not a metadata benchmark.)
5. Multi-slot behaviour under the host Eden cap as raised by the owner,
   and `allow_other` exposure accepted for every Eden slot (see the
   security section — the experiment's Eden mount itself also carried
   `allow_other`).

With those five, Option F is implementable as designed. The choice
between it and Option S remains the owner's.

## The choice, in one paragraph each

Option S asks for standing passwordless sudo for mount/btrfs/chown on this
host and gives kernel-speed mounts, btrfs quotas and reflink sharing, and no
new read exposure; it is the model already in use elsewhere. Option F asks
for no privilege at all and keeps each slot in its own sparse file, at the
price of `allow_other`'s any-local-user read exposure for Eden slots,
fuse2fs metadata speed, and the five unproven Eden behaviours above.
