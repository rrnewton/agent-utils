---
title: 'wrkslots-umount-ebusy-name-holder-and-retry: name the mount holder on EBUSY and retry transient holders'
status: closed
priority: 0
issue_type: bug
assignee: mu-removegaps
labels:
- wrkslots
created_at: 2026-11-16T16:30:20-05:00
updated_at: 2026-11-16T19:00:00-05:00
closed_at: 2026-11-16T19:00:00-05:00
---

# Description

[watermelon-full-preview] Field reports: a slot's umount fails EBUSY because a long-lived process's private mount namespace, created while the slot's bind existed, keeps a stacked copy. One such service restarts every minute (its private tmp clones the namespace). Remove treats one EBUSY as a final refusal and reports only "EBUSY", so the operator cannot tell a transient holder from a permanent one. Build on the private-bind work tracked separately; do not duplicate it.

# Design

- On EBUSY, name the holder: scan `/proc/*/mountinfo` for foreign namespaces that still hold the mount, and report pid, comm, the namespace id and the systemd unit if known.
- When every holder is transient (a namespace whose processes are younger than the slot's bind, or that disappears on re-check), retry the umount a bounded number of times, waiting on a re-check condition between tries, not a fixed sleep budget.
- Never kill or enter the holder.
- Python and Rust parity where the bind unmount exists in both editions.

# Acceptance Criteria

- A busy unmount whose holders are all transient succeeds on a bounded retry driven by re-checks, without any fixed sleep budget.
- A busy unmount that never clears names each holder's pid, comm, namespace id and unit when known.
- A non-transient holder is named and not retried into success; no holder process is killed or entered.
- Existing worktrees-image suites stay green in both editions.

# Notes

Depends on the private-bind change landing first; base on it rather than duplicating its privatise step.

## Resolution

[watermelon-full-preview] Landed as dbb1804e on main, on top of the private-bind change. A busy bind unmount now names each holder by pid, comm, mount-namespace id and unit from /proc mountinfo scans in both editions; a holder proven older than the bind fails fast with that naming, and transient holders are retried a bounded number of times waiting on holder re-checks instead of a fixed sleep budget. Holders are only read, never killed or entered. Validated and gated on the exact tree before landing.
