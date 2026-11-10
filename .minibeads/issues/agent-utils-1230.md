---
title: 'socket-liveness-proof: prove a filesystem socket node is dead before destructive recovery reclaims its tree'
status: open
priority: 2
issue_type: feature
assignee: cl-fence
created_at: 2026-11-10T17:46:18+00:00
updated_at: 2026-11-10T17:46:18+00:00
---

# Description

[opus 5.5] Destructive wrkslots recovery (`recover-ownerless-validate-batch` and the census behind remove) refuses every selected tree that contains a filesystem socket node: `_capture_lsof_process_path_census` reports "selected tree contains a filesystem socket node; socket-object liveness is unproven". A consumer's validation cleanup therefore never reclaims stale checkouts that hold dead sockets. The refusal is correct today, because a socket path's lstat device/inode and the sockfs inode numbers in /proc/PID/fd are different identity domains, so no numeric comparison can prove a socket has no owner. Bypassing it in the consumer would evade the shared destructive authority, so the proof belongs here.

Reported by a consumer coordinator with a design lead (not a tested proof). The existing alias controls in py/wrkslots/tests/test_lifecycle.py (relative bindings, hardlinks, an unlinked original, an original name rebound to another socket) must all be kept.

# Design

DESIGN FIRST, reviewed by the lead before any code. The design must settle:

- Evidence: a bounded NETLINK_SOCK_DIAG dump with UNIX_DIAG_VFS, giving the kernel's association between Unix socket identity and VFS (device, inode). Authenticate the kernel sender, request sequence and network namespace; parse the complete multipart response through DONE; refuse on errors, truncation, interrupted or incomplete dumps. Validate the device encoding and the interface's 32-bit VFS inode width; never truncate an inode and call it a nonmatch. Keep socket cookies where available.
- Composition with the existing generation-bound fd and network-namespace census: any matching owner or bound table entry refuses; an inaccessible namespace, unknown family, missing mapping, orphaned originating namespace, PID reuse or unreadable evidence also refuses. Keep the private-tree fence and the final same-UID closure model; no timestamp exemptions.
- Only after negative evidence, a bounded no-payload connect(). Success, timeout, a permission error, an unexpected errno or an unknown type refuses. ECONNREFUSED counts only with the same unchanged socket identity; ENOENT needs an explicit disappearance and re-census outcome. Check parent and path identity before and after; refuse replacements. Receipts distinguish a passive inventory from an active probe.
- A dry-run real-data adapter: normal-user reads only, deletes nothing, never constructs sudo or calls the root-owned census path; reports paths, distinct identities, dead/live/unknown counts and named reasons, with every visibility gap REFUSED.

# Acceptance Criteria

- Lead-approved design in Notes.
- Tests first: dead stale nodes, live listeners, live non-listening owners despite ECONNREFUSED, disappearance and rebinding, unreadable evidence, inode/device collisions, incomplete dumps, namespace churn; all existing alias controls kept; mutants that remove the listener, owner, visibility or identity checks, or swap the ENOENT/ECONNREFUSED meanings, are killed.
- Python first; a Rust parity note for the Rust edition's owner.
- Activation note: a consumer that loads its own pinned copy activates the change only when its pin moves.

# Notes

Owner: cl-fence (devbig030) after #1201 batch-grouped-fencing, which works in the same census and fence code (lead, 2026-11-10).
