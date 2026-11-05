---
title: 'wrkslots-census-exiting-pid: a PID exiting mid-census is re-probed instead of refused'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T08:49:54.496573472+00:00
updated_at: 2026-11-05T10:57:54.319512936+00:00
closed_at: 2026-11-05T10:57:54.319512826+00:00
---

# Description

`python.wrkslots.shard-6` was the second repeat offender in the 24 h gate ledger to 2026-11-05: 4 first-run failures out of 60 gates. It always passes solo.

The failure is the pre-census refusal "mount namespace is indeterminate for live PID N" (py/wrkslots/cli.py, the two `_mount_namespace` callers that raise it). Under host load, a process can be past the terminal and zombie checks but in the middle of exiting: `/proc/N/ns/mnt` is unreadable while `start_ticks` still reads. None of the existing race exits catch it, so the census refuses. The process is gone a moment later.

It was observed again in #872's cargo/absent family run: 3 failures under load, each passed solo.

This is a real robustness defect in the census, not only a test flake: the same race can refuse a production audit or remove on a busy host.

# Design

- When the mount namespace is unreadable for a PID that otherwise looks live, re-probe it a bounded number of times with a short backoff (for example 3 × 50 ms; keep the total small).
- Treat the PID as exited if any re-probe finds it terminal, a zombie, gone, or with a changed start time. Refuse only when it is still live and still indeterminate after the retries.
- Keep the refusal for a genuinely live, unreadable namespace (permission or a foreign user namespace). The retry must not turn that into acceptance.
- Mirror the same rule in the Rust edition if it has the equivalent census.

# Acceptance Criteria

- A unit test with a stubbed /proc reader models a PID whose namespace read fails once and that then becomes a zombie or disappears: the census continues.
- A PID that stays live and unreadable across the retries is still refused.
- shard-6's mount-namespace failures drop to zero first-run failures in the next 24 h ledger readout.

# Notes

From mu-validate's #827 readout preview (2026-11-05). Assigned to mu-validate after #874.
