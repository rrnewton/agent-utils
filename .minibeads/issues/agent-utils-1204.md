---
title: 'wrkslots-eden-checkouts: wrkslots support for EdenFS checkouts inside the btrfs image'
status: open
priority: 1
issue_type: task
depends_on:
  agent-utils-1423: blocks
created_at: 2026-11-10T12:41:39.328513866+00:00
updated_at: 2026-11-12T21:31:40.992079197+00:00
---

# Description

wrkslots can create, register, hold, inspect and remove an Eden checkout as a slot. It enforces the per-host cap of 3 Eden checkouts on this host. It places the Eden checkout, with its state, overlay and buck-out redirections, INSIDE the wrkslots btrfs image (worktrees.img on a loop mount, one subvolume per slot, 256 GiB qgroup per slot). The goal is to see whether Eden's btrfs metadata load moves into the image and off /data's main filesystem. Known traps: eden remove leaves buck-out redirections behind (once about 835 GB was orphaned on /data); freed btrfs metadata is not free space; du over an Eden mount can take forever (never du a mount; use eden's own du or stats). Phase 1 is survey and design only; no Eden checkout is created or removed until Phase 2 with coordinator approval.

# Design

Full design in ai_docs/transient/2026-11-10-wrkslots-eden-checkouts/DESIGN.md. Summary: reuse Checkout.vcs=eden for one Eden checkout per slot with the mount as a child of the slot subvolume; create refuses unless the worktrees image is verifiably mounted and the host-wide Eden count (union of eden list across the shared and per-slot config dirs) is under 3, then clones with a per-slot --config-dir inside the subvolume so overlay metadata is born in the image, establishes buck-out with an in-image target and verifies it via eden redirect list --json and mountinfo (create refuses until a target mechanism is proven in Phase 2); the shared backing repo stays outside. Remove salvages Sapling changes (sl status/id, patch plus draft bundle, verified read-back) for agent slots, runs eden remove, explicitly deletes recorded redirection targets and client state, and deletes the subvolume only after verifying the mount, binds, targets and orphaned-redirection accounting are clean. Census and holds extend to the mount, state dir and targets; audit meters usage only via eden du --json/stats, never by walking the mount. Measurement captures btrfs filesystem usage (Metadata Used and allocated) for /data and the image plus qgroup, eden du --json/stats and daemon memory before clone, after clone, after a build and after remove.

**Requirement: the Eden daemon is contained before any lane gets an Eden slot.** Each Eden checkout's daemon runs as its own systemd user unit (edenfs@<state-dir>.service), OUTSIDE the slot's and the gate's systemd-run scopes: the same escape class as a build daemon outliving its caller's scope. A real trial showed a du over the mount drove the uncontained daemon to 30 GiB of memory and 25.5 GiB of writes. So wrkslots must cap that unit itself, with a drop-in on the unit (CPUQuota, MemoryMax, Nice, IOSchedulingClass) or by moving it into a limited slice, and must verify the limits are in force before a lane is given the slot. This is BLOCKED on #1423 eden-daemon-containment (recorded as a blocking dependency). Until #1423 lands, the per-host Eden cap stays at 1, trials only, and no Eden mount is ever walked.

# Acceptance Criteria

Phase 1: design recorded in this issue (or linked ai_docs report) covering slot model, create/remove with verified redirection cleanup, host-wide cap enforcement, salvage, census/holds, and measurement plan; reported to coordinator before any implementation. Phase 2 (separate approval): trial Eden checkout inside the image on a freed slot, with before/after btrfs metadata measurement.

# Notes

Phase 2 answers (2026-11-10, mu-eden). 1 Daemons: one shared daemon holds 8.3 GB private (19.5 GB resident) for 3 checkouts, with one root-owned privhelper. A per-slot config dir therefore costs roughly one extra daemon plus privhelper per Eden slot (GBs, tens of seconds startup); Phase 2 fakes assume this and the real trial must measure it. The cap of 3 still holds because create counts checkouts, not daemons, and refuses at 3. 2 Cap discovery: enumerate config dirs from running edenfs processes (their --edenDir argument in /proc/*/cmdline), plus the edenDirectory from eden config and every wrkslots Eden slot config dir, run eden list per dir, dedupe by mount path; any dir that cannot be queried refuses creation (fail closed). 3 Size: 256 GiB bounds mount-visible writes plus overlay and buck-out inside the subvolume; a real build is expected to fit, but at the qgroup limit writes fail with quota errors and the FUSE checkout wedges until space is freed, so remove and audit must report quota exhaustion explicitly and never hang on the mount. 4 Remove: uncommitted Sapling work is preserved as a patch plus a bundle of the draft stack in the salvage archive with read-back verification, like git bundles; salvage failure refuses removal.

Owner input 2026-11-10 (reply 297) is folded into the design (DESIGN.md, Owner input section): cap becomes config eden_host_cap aiming for about 10 once metadata is isolated (Eden documented limit 10, guard 75/85 percent), Buck daemons pool by concurrency (max 2, per-root isolation, kill after build) with RE cache shared. Sized daemon measurement plan included; nothing measured yet. The Phase 2 code slice still hardcodes cap 3 and must be updated to the config value in the next slice.
