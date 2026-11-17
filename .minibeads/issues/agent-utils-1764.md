---
title: 'wrkslots-seal-identity-st-dev-after-reboot: accept a cross-boot seal identity when only the anonymous device changed'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T04:02:30.182619061+00:00
updated_at: 2026-11-17T06:08:19.451888459+00:00
closed_at: 2026-11-17T06:08:19.451888249+00:00
---

# Description

After a reboot, an interrupted validation-batch seal blocks every
mutation (transfer-owner, finish, remove, remove-validate-batch,
remove --validate-complete) with "interrupted validation-batch seal
recorded ... run 'wrkslots recover' first", and recover itself
refuses. The seal journal records the sealed directory's identity as
(dev, inode, file-handle type, file-handle bytes); after the reboot
only the device differs, because a btrfs subvolume's anonymous device
number is renumbered by a remount (the backing loop device is also
renumbered). The inode, handle type and handle bytes still match, and
the directory is still mode 0700 (sealed). The strict whole-tuple
comparison in the private-cleanup restore/re-fence path can therefore
never pass after a reboot, and the seal is unrecoverable. Separately,
a seal whose actor ran in an earlier boot on the same host is never
judged orphaned, because the orphan check refuses an earlier boot
outright, even though no process survives a reboot.

# Design

- In the seal restore/re-fence path only: when the seal actor's
  host identity matches this host and its boot differs from the
  current boot, accept the recorded identity when the inode, the
  file-handle type and the file-handle bytes match, both the recorded
  and the live device are anonymous (major 0), and a same-filesystem
  check that does not depend on the device number passes. In the
  same boot the exact strict comparison is unchanged.
- The stable filesystem identity is statfs's filesystem id, which
  folds the filesystem UUID with the subvolume root and survives a
  remount. New seal journals record it per target; the relaxed
  comparison then compares it exactly. A journal written before this
  change records none, so its target must instead prove it lives on
  the same filesystem as the registry's configured filesystem anchor
  (the project worktrees image mount when one is configured, else
  the validate root), judged from the current mount table (same
  mount source and type) or an equal filesystem id.
- A same-uid process cannot use the relaxed path to point cleanup at
  a different object: the file handle names one inode generation in
  one filesystem, so a recreated directory that reuses the inode
  number (every btrfs subvolume root is inode 256) has different
  handle bytes; a foreign filesystem (including a FUSE mount) fails
  the filesystem check and normally cannot mint the recorded handle
  at all; a bind mount of the same subvolume is the same object, and
  a bind mount of any other object fails the handle check. Any doubt
  (unreadable mount table, zero filesystem id, non-anonymous device,
  handle mismatch) refuses.
- A seal actor on the same host identity with an earlier boot is
  dead regardless of any pid check, so the orphan judgement retires
  such a seal (create's auto-recovery and recover included). The
  rule stays small and self-contained in the orphan check so the
  batch-operator liveness work can rebase onto it.
- The Rust edition validates the new journal field where it parses
  seal journals; it has no seal restore path of its own.

# Acceptance Criteria

- Deterministic regression tests fake a different boot and a changed
  anonymous device: recover of the exact reported seal shape succeeds
  (fails before, passes after).
- Negative tests: same boot with a changed device still refuses; a
  different inode or handle still refuses; a different filesystem
  still refuses.
- New seal journals record the stable filesystem identity.
- Focused wrkslots tests pass in both editions where they implement
  the behaviour; the full local validate passes.

# Notes

Reported from a live registry after a host reboot; the fix is proven
with fixtures that reproduce the recorded journal shape, never by
touching that registry.

## Resolution

Landed on main as c543a2bdc87dc28d328f5b0be0b9255820033f2e (gate
green). The seal restore/re-fence path now accepts a cross-boot
identity on inode, handle type and handle bytes plus a proven same
filesystem (recorded stable filesystem id in new journals; the
configured anchor for older ones), an earlier-boot same-host actor is
dead for the orphan judgement, and the Rust journal parser accepts
the new per-target filesystem id. Regression tests cover the reported
shape and the same-boot, different-handle, different-inode and
different-filesystem refusals.
