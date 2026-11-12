# #1409 eden-fuse-image: the sudo worktrees-image path proven on this host

Status: experiment complete, 2026-11-12. The owner chose Option S from
`2026-11-12-eden-fuse-image.md`: a project-wide `worktrees_image` (one
loop-mounted btrfs image, one quota'd subvolume per slot) with `sudo -n`
only through wrkslots' own helpers for image create, mount, unmount and
teardown. fuse2fs remains only a fallback. This note records the proof
on this host, the one gap it found, and the adoption recipe for a
project team to apply to its own configuration. No consumer is named;
teams are labelled neutrally (`consumer-a` style) where needed.

## Experiment (scratch project, real Eden clone)

A scratch wrkslots project under `/tmp/edensudo-exp/project` (never a
live harness configuration and never another team's registry):

- `.wrkslots.yml` with `worktrees_image: {path: worktrees.img,
  size_bytes: 10737418240, slot_quota_bytes: 4294967296,
  machines: [devbig030]}`, `layout: flat`, `eden_host_cap: 6` (the
  scratch project only; the raised cap is what lets a fourth checkout
  exist beside the host's three).
- `wrkslots worktrees-image create` made the sparse image (10 GiB
  apparent, 4.3 MiB allocated after `mkfs.btrfs`); `worktrees-image
  mount` loop-mounted it at `worktrees/.slots-image` (quota mode
  `simple` reported by `worktrees-image status`).
- `wrkslots create edenprobe --eden main=<backing repo>` ran inside a
  `systemd-run --user` scope (`CPUQuota=800%`, `MemoryMax=16G`,
  nice 19, idle IO). The slot directory is a btrfs subvolume (inode
  256) bind-mounted from the image; the Eden checkout mounted inside
  it; `sl -R <checkout> status` exited 0 clean; a file read from the
  checkout returned its content.

Commands that used sudo, all through wrkslots' `sudo -n` helpers
(`worktreesimage._run`), never ad-hoc: `mkfs.btrfs` at image create;
`mount -o loop` at mount; `btrfs subvolume create` and `chown` at
slot create; `mount --bind` for the slot bind; `umount` at teardown;
`btrfs subvolume delete` for the slot subvolume. The Eden clone,
`sl status` and file reads used no sudo.

### Device proof and buck-out

- Slot: `stat` device 31457291, inode 256 (subvolume root);
  `findmnt` shows the slot path as `/dev/loop52[/edenprobe]` (btrfs),
  and the checkout as `edenfs:` (fuse) inside it.
- `eden redirect list --json` reports the `buck-out` target as the
  daemon-derived host scratch path; that path is a symlink into the
  slot (`<slot>/buck-out-target`), and `findmnt -T
  <checkout>/buck-out` resolves to `/dev/loop52[/edenprobe/buck-out-target]`
  (btrfs) — the same device as the slot. The #1204 retarget keeps
  buck-out physically inside the image; nothing was written to host
  scratch beyond the symlink itself.
- Peak real allocation of the image file: 269 MiB (`du` of the image
  file itself, measured at teardown), against the 10 GiB apparent
  size. Expected footprint was under 1 GiB real; the clone is lazy
  and only the files read were materialised. Measure an image by its
  backing file, never by `du` over an Eden mount: that walks the
  whole virtual checkout, wakes the daemon into materialising it,
  and is how this run's daemon ballooned before teardown.

### Containment finding

`eden clone` does not stay inside the caller's `systemd-run` scope:
the daemon starts as its own user service, `edenfs@<state dir>.service`,
at Nice 0 with no CPUQuota and no MemoryMax, however the clone was
launched. Bounded Eden work therefore needs containment on the
daemon itself — a drop-in on `edenfs@.service` or Eden's own
configuration — not only a scope around the creating command. Any
adoption of Eden slots under a host load policy must include that
drop-in; a scope around `wrkslots create` alone does not bound the
daemon that outlives it. Tracked as the open follow-up
#1423 eden-daemon-containment, not as work inside #1409.

### Teardown

`eden remove -y <checkout>` succeeded; `eden stop` for the slot's
config dir reported a clean exit; the `edenfs@` unit for the probe
state dir is inactive, `eden list` for that config dir is empty,
and no process names the scratch tree. The slot subvolume and bind
were removed through wrkslots' helpers (`unbind_slot`,
`delete_slot_subvolume`), `wrkslots worktrees-image unmount`
unmounted the image, and the scratch project was deleted. Verified
afterwards: no scratch path in `findmnt`, no loop device for the
scratch image in `losetup -a` (the remaining loops are the harness's
own slot images, untouched), `/tmp/edensudo-exp` gone. The host's
pre-existing Eden checkouts and daemon were never modified.

## Gap found and fixed

`wrkslots remove` of the scratch Eden slot refused before any Eden
step: `_registry_storage_inconsistencies` judged the checkout's
external backing repo by the Git sibling rule ("stored absolute
repository path must name one direct sibling of the project root").
Eden backing repos live outside the project by design, so every Eden
row tripped it. The check now skips `vcs == "eden"` checkouts (the
daemon checks in the Eden paths judge them), with a regression test
(`test_storage_consistency_skips_eden_backing`, stubbed facts, no real
sudo and no real daemon). The scratch teardown therefore used the
sequence in Wanted: `eden remove`, daemon-gone verification, subvolume
and bind removal through wrkslots' helpers, image unmount, image file
deleted. A full `wrkslots remove` of an Eden slot still needs its
normal handoff/owner lifecycle; that is policy, not this gap.

The `eden stop` caveat from the fuse note stands: after `eden
remove`, verify the per-slot daemon is actually gone (its
`--edenDir` no longer in any process cmdline and the image
unmountable) rather than trusting the stop result.

## Adoption recipe (apply to your own config)

For a project team enabling this on this host, in its own
`.wrkslots.yml`:

```yaml
eden_host_cap: 6
worktrees_image:
  path: worktrees.img          # project-relative, outside the mount
  size_bytes: 1099511627776    # 1 TiB apparent; sparse, an upper bound
  slot_quota_bytes: 68719476736  # 64 GiB per slot, referenced bytes
  machines: [devbig030]        # only the host(s) holding the image
```

Size guidance: the apparent size is a ceiling, not a reservation;
pick it for the fleet's total working set, and grow online with
`wrkslots worktrees-image grow --to-size-bytes N`. The per-slot quota
bounds each slot's referenced bytes inside the image. Raise
`eden_host_cap` only as far as the host's Eden load allows; it counts
every checkout on the host across all daemon config dirs, inside and
outside images.

Then, with the project's own wrkslots (at or after the Eden removal
sequencing change):

```sh
wrkslots worktrees-image create   # once; sudo mkfs.btrfs via wrkslots
wrkslots worktrees-image mount    # sudo loop mount + binds
wrkslots create <slot> --slot-type agent --coordinator-authorized \
  --agent <agent> --task <task> --purpose <text> \
  --owner-pid <pid> --coordinator-pid <pid> \
  --eden main=<backing repo path>
wrkslots worktrees-image status   # state=mounted, quota mode, bytes
```

Removal follows the ordinary slot lifecycle; the Eden sequence
(salvage, `eden remove`, verified orphan checks, then storage
teardown) runs before any Git path fence. Until image create and
mount have run, slot-writing commands refuse rather than write onto
the host underneath the mount point — that refusal is expected, not
an error to work around.
