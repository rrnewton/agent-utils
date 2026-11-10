---
title: 'wrkslots-eden-checkouts: wrkslots support for EdenFS checkouts inside the btrfs image'
status: open
priority: 1
issue_type: task
created_at: 2026-11-10T12:41:39.328513866+00:00
updated_at: 2026-11-10T12:41:39.328513866+00:00
---

# Description

wrkslots can create, register, hold, inspect and remove an Eden checkout as a slot. It enforces the per-host cap of 3 Eden checkouts on this host. It places the Eden checkout, with its state, overlay and buck-out redirections, INSIDE the wrkslots btrfs image (worktrees.img on a loop mount, one subvolume per slot, 256 GiB qgroup per slot). The goal is to see whether Eden's btrfs metadata load moves into the image and off /data's main filesystem. Known traps: eden remove leaves buck-out redirections behind (once about 835 GB was orphaned on /data); freed btrfs metadata is not free space; du over an Eden mount can take forever (never du a mount; use eden's own du or stats). Phase 1 is survey and design only; no Eden checkout is created or removed until Phase 2 with coordinator approval.

# Design

To be completed in Phase 1 survey: slot model for an Eden checkout as a new checkout kind next to git worktrees; create/remove flows with redirection cleanup verified; cap enforcement counting every Eden checkout on the host; salvage of uncommitted Sapling changes before remove; census and holds; measurement plan for btrfs metadata on /data before and after and image usage.

# Acceptance Criteria

Phase 1: design recorded in this issue (or linked ai_docs report) covering slot model, create/remove with verified redirection cleanup, host-wide cap enforcement, salvage, census/holds, and measurement plan; reported to coordinator before any implementation. Phase 2 (separate approval): trial Eden checkout inside the image on a freed slot, with before/after btrfs metadata measurement.
