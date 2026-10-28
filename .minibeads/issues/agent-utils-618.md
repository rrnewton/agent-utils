---
title: 'wrkslots-census-foreign-procs: slot-use census must not refuse on unrelated processes unreadable fds'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-28T10:46:28.888320719+00:00
updated_at: 2026-10-28T12:59:50.113598161+00:00
closed_at: 2026-10-28T12:59:50.113598010+00:00
---

# Description

The Python wrkslots process-use census refuses the whole remove/validate operation as 'process use is indeterminate' when readlink on ANY host process's fd fails, e.g. ENAMETOOLONG on /proc/1186775/fd/26 (an unrelated test binary) or a stuck PID-namespace init (PID 2991802, zap_pid_ns_processes). This blocks slot removal for processes unrelated to the slot (observed in gate 52 of #562 slice 36: packages.standalone.wrkslots, python.wrkslots.shard-0, python-lifecycle.host.shard-0/1 on the host; all pass in a PID namespace).

Do NOT simply treat unreadable fds as not-using-the-slot: a very long path inside the slot could also raise ENAMETOOLONG and fail open. Instead, when readlink fails, stat /proc/PID/fd/N (stat follows the fd even when readlink cannot) and compare st_dev with the slot's filesystem device. Slots are separate ext4 images, so a different st_dev proves the fd is elsewhere. Keep 'indeterminate' only when stat also fails.

Scope: py/wrkslots/cli.py census (_readable_link_target and callers), with tests: a foreign fd whose readlink fails but st_dev differs from the slot device must not refuse; a same-device unreadable fd must still refuse. Rust wrkslotsd use-scan parity follows in the same fix if it shares the failure mode. Filed by mu-rust at coordinator direction after #562 slice 36.

## Resolution (2026-10-28, mu-rust)

Landed on main: `ffbfd7cf` (device proof in `_readable_link_target` and `_agent_batch_fresh_use`), `a9490079` (sibling `_link_target` parity; Rust use-scan errno tolerance, deleted-suffix trim, unstatable-slot refusal), `c8cc2135`, `b418d606` (test and typing fixes). When readlink fails outside the no-evidence errnos, both censuses now stat the descriptor: a different `st_dev` than the slot's is proven outside the slot; same-device or failed stat stays indeterminate (fail-closed, per the design note in this issue).

Tests: `py/wrkslots/tests/test_census_foreign_fds.py`, 6 passed, including fresh-use integration on a second device (`/dev/shm`) for fd and cwd links; Rust `link_points_into` unit tests pass; `wrkslotsd` 89 package tests pass.

Verified on this host: `test_remove_agent_batch_checks_an_image_backed_slot_with_lsof_before_its_fence` now passes. Residual, by design: when the foreign descriptor's target sits on the SAME device as a plain (non-image) fixture slot under /tmp, the proof cannot distinguish it, so `packages.standalone.wrkslots` and the Rust remove parity tests still refuse on this host while unrelated PID 1186775 lives; they pass in a PID namespace, and production slots are separate filesystem images where the device proof applies. The other stuck process (PID 2991802, PID-namespace init) affects a different validation census and is out of this issue's scope.
