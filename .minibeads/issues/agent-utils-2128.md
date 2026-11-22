---
title: 'wrkslots-seal-recover-scoped-census: recovering a dead validation-batch seal must terminate on a busy host'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T14:45:00-05:00
updated_at: 2026-11-22T14:45:00-05:00
---

# Description

A coordinator recover of a dead validation-batch seal (its actor process gone) restored the seal and then refused: "same-UID process census did not close: each of 8 process inventories found generations that forked or exited before their scan completed". While the seal stands, every validation launch on the host is refused, so one busy host blocks all validation for every consumer. Reported before on 2026-11-19.

# Design

Owner direction: scoped notions, not host-wide process scans. The seal recovery decides only whether anything still uses the batch's own validation checkouts; scope that to the batch's recorded run cgroups (cgroup.events populated 0 means empty, as in #2092) or to the checkouts' image mounts (a non-lazy unmount succeeds), and never census every same-UID process on the host. Process listing only names a holder in a refusal. The decision must terminate in bounded time regardless of host load. Owner rule for every lock holder: constant-time work under the lock; linear work prepared outside and validated under the lock with an O(1) generation check.

## Refinement (mu-sealcensus, 2026-11-22)

The refusal is the paired-finish recovery path: `_finish_remove_paths`
runs `_capture_same_uid_process_path_census` on the fenced slot just
before deletion. Implemented (after cold review): on that path only,
a validate slot whose recorded owner generation is proven dead is
decided by `_scoped_validation_run_cgroup_state` — the recorded
owner cgroup (one direct path, constant reads) must read
`cgroup.events` `populated 0` or be absent (the service manager
removes a dead owner's scope); nonempty refuses naming the first
`cgroup.procs` member, and present-but-unreadable refuses fail-closed.
The census then runs once as a reference scan: a completed scan's
matches refuse as before, but a census that cannot close under host
churn no longer blocks recovery. No persisted-format change and no
new event kind or key. Scoped auto-recovery of seals whose remover
is unprovable by PID (consumer bead dh-wmd22n) was rejected in cold
review and is not implemented; auto-recovery remains proven-dead
operator only. The Rust port follows this Python spec separately.

# Acceptance Criteria

- A seal whose actor is dead and whose recorded owner cgroup is empty (or absent on a unified cgroup2 hierarchy) is recovered in bounded time on a host with heavy unrelated process churn (test with a churn generator); a live holder inside the batch cgroup still refuses.
- A batch cgroup that is present but unreadable refuses fail-closed on the seal-recovery path; a census that cannot close under churn, and found nothing, no longer blocks recovery, while any census refusal carrying a match still refuses.

# Notes

P0: it blocks every validation launch host-wide. Filed by the coordinator from a consumer report.
