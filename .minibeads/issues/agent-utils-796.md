---
title: 'wrkslots-reclaim-slot-state-homes: removing a slot leaves its per-slot state home behind'
status: open
priority: 1
assignee: mu-rust@devbig030
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-02T23:49:09.491856250+00:00
updated_at: 2026-11-02T23:49:09.491856250+00:00
---

# Description

[opus 5.5] Reported by a consumer team 2026-11-02 (disk; the owner wants a deterministic reclaim protocol): removing or retiring a slot leaves its per-slot home under worktrees/slot-state/agent/<slot> behind. On one host, 22 such homes for already-removed slots held ~168 GB (largest 35 GB, 18 GB, 18 GB, 11 GB); no wrkslots command reclaims them, so they were checked and deleted by hand. Asks: (1) slot removal and finish-with-removal also reclaim the slot's slot-state home, under the same safety checks (no live process; unpushed git saved as a bundle/rescue ref); (2) 'wrkslots reclaim-orphan-state' (read-only plan by default, --apply to act) for existing leftovers, keyed to the archived slot records; (3) both respect the #780 retire freeze.
