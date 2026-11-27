---
title: 'wrkslotsd-snapshot-census-ignores-gone-generations: the Rust process snapshot closes by ignoring generations that exit before their read'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- rust
- census
- safety
created_at: 2026-11-27T08:40:00-05:00
updated_at: 2026-11-27T08:40:00-05:00
---

# Description

Safety. The Rust process snapshot (`capture_process_snapshot_at` in `rs/wrkslotsd/src/remove_write.rs`) counts a listing as proven when none of its new generations uses, or may use, a target, and skips a process that exits while it is read. As in the Python all-process census (#2379), that is the closure rule the 2026-11-19 cold review rejected for the same-UID census (`b1f7255e`, reverted by `8a1629b1`/`ad03f320`): a chain of fork-then-exit hand-offs, one per listing, gets past two proven listings, so the snapshot can close while a descendant of a holder still uses the target.

Callers today: `assert_slot_unused` (remove), `audit_record`, `wt_migrate_cli`, `classify_ownerless_validate_batch`, `assert_excluded_unused` and `frozen_release_dry_run`.

Found while designing #2376 wrkslots-census-hot-host (design note `ai_docs/plans/2026-11-27-wrkslots-census-hot-host-design.md` in the harness repository, section (a)).

# Design

Port the PID-cursor bracket from #2376 to the snapshot: probe every PID number allocated after the first listing (cursor from `/proc/loadavg`) and close only when every live process has been read clean and the cursor stood still for two whole probe steps; when the cursor cannot be used (not in the initial PID namespace, unreadable `/proc/loadavg`), keep today's rule. Keep the existing `proc_root` test seam and add seams for the cursor and the probe. Both editions must agree, so the Python change in #2379 and this one share one differential.

# Acceptance Criteria

- A Rust unit test with a fake proc root: a holder forks its heir and exits before its own read in each listing; the snapshot never closes (today it closes).
- The existing churn tests (`processes_exiting_during_the_snapshot_never_refuse_or_hide_a_user` and the rest) still pass.
- A Python/Rust differential over the same scripted host gives the same verdicts; no persisted field changes.

# Notes

Filed by cl-census at coordb's direction (number assigned by coordb). Sequenced after #2376 and with #2379.
