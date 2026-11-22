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

# Acceptance Criteria

- A seal whose actor is dead and whose run cgroups are empty is recovered in bounded time on a host with heavy unrelated process churn (test with a churn generator); a live holder inside the batch cgroup still refuses; Python and Rust agree.

# Notes

P0: it blocks every validation launch host-wide. Filed by the coordinator from a consumer report.
