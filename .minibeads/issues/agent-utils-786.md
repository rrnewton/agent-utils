---
title: 'agentctl-start-slot-bind-busy-lock: start drops the slot-owner bind when the wrkslots lock is busy and still returns 0'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T18:47:10.938157572+00:00
updated_at: 2026-11-02T18:47:10.938157572+00:00
---

# Description

[opus 5.5] Reported by a consumer team on build f0ec02a1 while starting root-isolated Claude lanes. (1) Under slot_isolation=root, 'agentctl start' drops the slot-owner bind when the wrkslots lock is busy: in crate::restore::bind_slot_owner, the 'wrkslots status' read in slot_row() has no --wait-lock and fails with 'REFUSED: state lock is busy'. (2) start still returns rc 0 with slot_owner.action=refused, easy to miss: retry with a bounded wait (status --wait-lock; transfer-owner tolerating a busy config lock), then fail loudly if the bind still fails. (3) No agentctl subcommand redoes the bind afterwards; inside a root box 'wrkslots adopt' fails ('cannot open state lock', registry read-only). Workaround from outside the box: 'wrkslots --wait-lock 30 transfer-owner SLOT --agent A --owner-pid P --expected-generation N' (needed one retry on a busy config lock). Provide 'agentctl rebind-slot NAME' (or similar) that does this from outside. Related: a Muse lane under cgroup isolation saw 'slot adopt refused (live historical owner)' in its slot; check it in the same pass. Overlaps #744 (wrkslots re-acquire of a released generation).
