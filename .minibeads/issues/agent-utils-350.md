---
title: 'wrkslots-foreign-mount-ns: slot image mounts copied into unrelated mount namespaces block removal, defeat mount identity, and get mounted twice'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-18T21:50:31.687706272+00:00
updated_at: 2026-10-18T23:05:52.175402756+00:00
closed_at: 2026-10-18T23:05:52.175402596+00:00
---

# Description

[opus 5.5] Image-backed slots (kernel backend) misbehave when unrelated sandboxed processes create private mount namespaces while slot images are mounted. Each such namespace keeps its own copy of every slot mount. Unmounting on the host does not propagate into it, so the copy keeps the loop device and the image file referenced.

Observed: several slots held this way by processes in two private mount namespaces (each process is PID 1 of its own user and PID namespaces). None of them has a working directory, root, open file or mapping inside any slot. They only hold copied mount-table entries.

1. remove refuses "live process <pid> uses slot <path>: mount=<slot path>". The census treats any mount line under the slot as use, except the slot's own image mount, and it recognises that mount by the loop device's backing-file path as sysfs renders it. That rendering stops resolving once the mount namespace that attached the loop device has exited: it is then shown relative to a detached mount's root. This is systemic, not specific to foreign namespaces. Every image slot mounted from a per-session mount namespace becomes unrecognisable when that session exits, so the slot's own mount is not recognised even on the host. The same comparison makes unmount return as if nothing were mounted (converting the slot to a plain worktree then deletes an image that is still mounted), makes the root helper's umount, trim and grow fail, and makes every later command warn "slot image not mounted: <state mount> already has a different file system mounted on it".
2. The image mount routine always attaches a new loop device. After the host unmounted a slot that a foreign namespace still held, a later remount put a second, independent ext4 file system on the same image file. One slot is in that state now. Two superblocks on one image can corrupt it: the stale copy writes its superblock and journal when it is finally released, and lazy inode-table initialisation can run under both.
3. 'image grow' on an image that is not mounted on the host takes the offline path (e2fsck and resize2fs on the image file), even while a foreign namespace still mounts it through a loop device.
4. Automatic remount (every command, and 'wrkslots image mount') stops at the first slot that fails. The remaining unmounted slots stay unmounted.
5. While a slot's image is unmounted, its mount point is an empty directory inside the project. Git resolves the checkout to the enclosing repository, and the storage check reports "checkout does not identify the recorded repository worktree". Removing another slot of the same repository then repaired that slot's Git registration with 'git worktree repair', which, run in the main worktree, rewrites the .git file of every linked worktree whose directory exists. It wrote a host-side .git file into the empty mount points of the unmounted slots. Those mount points are now non-empty, so their images can no longer be mounted, and Git sees each of those worktrees with every file deleted.

Expected:
- A loop mount is recognised as a slot's own image by its device number and the file system UUID read from the image file, never by a rendered backing path. A backing file shown as deleted never matches a new image at the same path.
- An image already attached to a loop device is never attached to a second one. The existing device is reused (re-checked on an open descriptor that is held across the mount), or the mount refuses.
- A copy of the slot's own mount (same device, whole file system, at the slot's recorded path) in any namespace is storage, not use. Every process-level check (working directory, root, open files, mappings, owner cgroup) still runs for every process, including processes in that namespace.
- A double-attached image is reported by doctor, is never attached again, and is removed only with --coordinator-authorized --discard-stale-image-copy, which records the basis and every copy seen in the event log.
- Unmount refuses when a loop mount sits at the mount point but cannot be identified. It never returns as if nothing were mounted.
- Offline image tools (grow) refuse while any loop device holds the image, and name the device.
- Automatic remount continues past a failing slot and reports each failure. 'image mount' mounts the rest and exits 3, listing every failure.
- Repairing one checkout's Git registration never writes into another checkout's directory.
- No Git inspection or repair runs against an image-backed checkout whose image is not mounted. Doctor and status report image-not-mounted for that row instead of a Git mismatch, and it does not block commands on other rows.
- A host-side .git file at an image mount point that is exactly Git's pointer to the same worktree registration is removed only by 'image mount --coordinator-authorized', and the removal is recorded in the event log.

Acceptance: unit tests without root for each expected point, and an opt-in root end-to-end test with a real loop mount and an idle private-namespace copy that (a) removes the slot with the copy present, after which no namespace still mounts the slot and the loop device is released; (b) refuses while a process in that namespace has its working directory inside the slot; (c) reuses the attached device when remounting after a host-only unmount; (d) reports a second attachment forced with 'losetup -f' plus mount, and does not remove that slot without explicit authorization. The end-to-end test needs passwordless sudo and unprivileged user namespaces, which agent sandboxes refuse, so the owner or a CI job with both runs it.
