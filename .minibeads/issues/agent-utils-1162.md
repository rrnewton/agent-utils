---
title: "worktrees-image-at-slots-dir: mount the worktrees image at worktrees_dir with slot subvolumes as top-level entries and no binds"
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T20:45:00.000000000+00:00
updated_at: 2026-11-09T20:45:00.000000000+00:00
---

# Description

The worktrees image (the `worktrees_image` section) mounts one sparse btrfs
image at `<control>/.slots-image` and bind-mounts every slot subvolume at its
slot path. Loading the configuration re-establishes every recorded bind
(`ensure_binds`), so each slot costs a mount, the bind record has to be kept
in exact agreement with the live mount table, and a removal fence has to
relocate a bind because a mount point cannot be renamed.

The owner and the first adopter (a consumer) want the simpler layout the
module originally described: mount the image at the slots directory itself
(`worktrees_dir`, for example `worktrees/slots`). Slot subvolumes are then
simply the image's top-level entries; there are no bind mounts and no bind
records. The adopter wants to switch now and mount the image from its own
boot unit, which today's code fights when `worktrees_image` is configured: a
config load mounts the image a second time at `.slots-image` and stacks a
bind of each subvolume onto itself, then create and remove break on mkdir,
rename and rmdir of a subvolume root. With the section removed, slot
creation silently makes plain directories without a quota, nothing refuses a
write into an unmounted empty slots directory, and removal of an existing
subvolume slot depends on rmdir deleting a subvolume (kernel dependent).

Two related needs arrived with this request. A consumer worried about btrfs
slowdowns with many subvolumes wants simple quotas by default (kernel 6.7+):
plain `btrfs quota enable` runs at image create and on every mount today,
with no way to ask for `--simple` and no report of which mode is active.
Separately, when the image is unmounted every slot path looks missing, so a
`git worktree prune` in the primary repository would detach every slot's
worktree registration; the adopter locks each registration by hand, after
which `wrkslots` removal refuses it because it passes a single `--force`.

# Design

Add `mount_at` inside `worktrees_image`: `binds` (the current layout, the
default) or `worktrees_dir` (direct: the image's top level is
`worktrees_dir`). The key lives inside the section so #1152
worktrees-image-host-scope can still wrap the whole section per host. In
direct mode: mount and classify at `worktrees_dir` and chown the image root
to the user so fence renames work; `ensure_binds` and `bind_slot` return
early and no bind records are read or written; create makes a subvolume plus
qgroup limit directly at `worktrees_dir/<slot>`; remove fences by renaming
the subvolume and deletes it with `btrfs subvolume delete`; image mount,
unmount, status, grow and trim use `worktrees_dir`; doctor and audit know
the mount. Validate slots move inside the image at
`worktrees_dir/validate/<slot>` (a plain directory holding one subvolume per
validate slot; the agent slot name `validate` is refused in direct mode);
bind mode keeps `worktrees/validate`.

Add `wrkslots worktrees-image migrate-layout --to direct|binds` converting
between the layouts: it refuses while any slot (agent or validate) is in
use, using the same census as the removal fence, unbinds or rebinds every
slot, rewrites the configuration only on success, and for this direction
absorbs #1149 worktrees-image-migrate-original and #1150
worktrees-image-migrate-liveness (reclaim the original the way the removal
path does; the host-to-image `migrate` command's own defects stay with
those issues).

Add `quota_mode` inside `worktrees_image`: `simple` (default where the
kernel and btrfs-progs support it, falling back to full qgroups with a
notice) or `full`, applied when the image is created and first mounted.
Mounting must never flip an existing mode (verified: plain `quota enable`
on a simple-quota filesystem is a no-op, including through remount).
`worktrees-image status` reports the active mode: none, full or simple.
Document conversion of an existing image (disable, `enable --simple`,
re-apply every slot limit, which disable drops; extents written before the
switch are uncounted until rewritten). No convert subcommand: documented
commands only, because conversion is a one-time operator act and the
under-count makes automation misleading.

In both layouts, `worktrees_image` takes `git worktree lock --reason "in
worktrees image"` at create and at migrate so an unmounted image can never
be pruned away. Removal and recovery drop only wrkslots' own lock (exact
reason match) after the fence, before `git worktree remove`; a lock with
any other reason still refuses as today. Image status and doctor report
image slots whose lock is missing.

Rust readers (`rs/wrkslotsd/src/worktrees_image.rs` from #1141) must at
least parse `mount_at` and `quota_mode` and not refuse the configuration.

# Acceptance Criteria

- Direct mode end to end on a real loop-mounted btrfs scratch image as the
  existing image tests do: create makes a quota-limited subvolume at the
  slot path, remove deletes it after the fence, unmounted `worktrees_dir`
  refuses slot writes, status reports the direct mount point and quota mode.
- A migration round trip binds to direct to binds preserves slot contents,
  refuses with a live holder in a slot, and leaves configuration and mounts
  consistent after refusal.
- `git worktree prune` with the image unmounted leaves locked slots intact;
  remove succeeds on a wrkslots lock and refuses on a foreign lock.
- A new image gets `quota enable --simple` where supported (status says
  simple; a slot limit enforces), full qgroups with a notice where not, and
  a remount never changes the mode.
- Bind mode behaviour unchanged: existing worktrees-image tests pass,
  including the #1147 locked bind-record path and the #1142 removal fence.
- The USER_GUIDE and `--help` describe `mount_at`, `quota_mode`, the
  validate layout, quota conversion and the worktree lock.

# Notes

Owner request 2026-11-09 relayed from the first adopter (a consumer), which
is switching to a boot-unit mount at `worktrees_dir` today with the
`worktrees_image` section removed, under caveats (plain directories for new
slots, manual unlock before remove, manual subvolume delete where rmdir
refuses). Runs first on current main: #1147 (landed a80e9010, touched here
only with direct-mode early returns), then this issue, then #1148
worktrees-image-silent-mount-failure and the host-to-image parts of #1149
and #1150, then #1152 host scoping. Builds on #1142 and #1153 removal
ordering (fence first, then shared salvage bundles). Quota facts verified by
the adopter on kernel 7.1.3 with btrfs-progs 6.7 (no mode flip on plain
enable; qgroup limit enforces under simple quotas; disable drops limits;
pre-switch data uncounted).
