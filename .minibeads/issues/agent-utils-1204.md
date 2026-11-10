---
title: 'wrkslots-eden-checkouts: wrkslots support for EdenFS checkouts inside the btrfs image'
status: open
priority: 1
issue_type: task
created_at: 2026-11-10T12:41:39.328513866+00:00
updated_at: 2026-11-10T12:52:06.282677897+00:00
---

# Description

wrkslots can create, register, hold, inspect and remove an Eden checkout as a slot. It enforces the per-host cap of 3 Eden checkouts on this host. It places the Eden checkout, with its state, overlay and buck-out redirections, INSIDE the wrkslots btrfs image (worktrees.img on a loop mount, one subvolume per slot, 256 GiB qgroup per slot). The goal is to see whether Eden's btrfs metadata load moves into the image and off /data's main filesystem. Known traps: eden remove leaves buck-out redirections behind (once about 835 GB was orphaned on /data); freed btrfs metadata is not free space; du over an Eden mount can take forever (never du a mount; use eden's own du or stats). Phase 1 is survey and design only; no Eden checkout is created or removed until Phase 2 with coordinator approval.

# Design

Full design in ai_docs/transient/2026-11-10-wrkslots-eden-checkouts/DESIGN.md. Summary: reuse Checkout.vcs=eden for one Eden checkout per slot with the mount as a child of the slot subvolume; create refuses unless the worktrees image is verifiably mounted and the host-wide Eden count (union of eden list across the shared and per-slot config dirs) is under 3, then clones with a per-slot --config-dir inside the subvolume so overlay metadata is born in the image, establishes buck-out with an in-image target and verifies it via eden redirect list --json and mountinfo (create refuses until a target mechanism is proven in Phase 2); the shared backing repo stays outside. Remove salvages Sapling changes (sl status/id, patch plus draft bundle, verified read-back) for agent slots, runs eden remove, explicitly deletes recorded redirection targets and client state, and deletes the subvolume only after verifying the mount, binds, targets and orphaned-redirection accounting are clean. Census and holds extend to the mount, state dir and targets; audit meters usage only via eden du --json/stats, never by walking the mount. Measurement captures btrfs filesystem usage (Metadata Used and allocated) for /data and the image plus qgroup, eden du --json/stats and daemon memory before clone, after clone, after a build and after remove.

# Acceptance Criteria

Phase 1: design recorded in this issue (or linked ai_docs report) covering slot model, create/remove with verified redirection cleanup, host-wide cap enforcement, salvage, census/holds, and measurement plan; reported to coordinator before any implementation. Phase 2 (separate approval): trial Eden checkout inside the image on a freed slot, with before/after btrfs metadata measurement.

# Notes

Phase 2 answers (2026-11-10, mu-eden). 1 Daemons: one shared daemon holds 8.3 GB private (19.5 GB resident) for 3 checkouts, with one root-owned privhelper. A per-slot config dir therefore costs roughly one extra daemon plus privhelper per Eden slot (GBs, tens of seconds startup); Phase 2 fakes assume this and the real trial must measure it. The cap of 3 still holds because create counts checkouts, not daemons, and refuses at 3. 2 Cap discovery: enumerate config dirs from running edenfs processes (their --edenDir argument in /proc/*/cmdline), plus the edenDirectory from eden config and every wrkslots Eden slot config dir, run eden list per dir, dedupe by mount path; any dir that cannot be queried refuses creation (fail closed). 3 Size: 256 GiB bounds mount-visible writes plus overlay and buck-out inside the subvolume; a real build is expected to fit, but at the qgroup limit writes fail with quota errors and the FUSE checkout wedges until space is freed, so remove and audit must report quota exhaustion explicitly and never hang on the mount. 4 Remove: uncommitted Sapling work is preserved as a patch plus a bundle of the draft stack in the salvage archive with read-back verification, like git bundles; salvage failure refuses removal.
