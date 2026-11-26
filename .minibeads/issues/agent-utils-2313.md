---
title: 'wrkslots-eden-census-skips-owner-default-instance: never query the default Eden instance in the host cap census'
status: closed
priority: 1
issue_type: bug
assignee: cl-edencensus
labels:
- wrkslots
created_at: 2026-11-26T21:35:00+00:00
updated_at: 2026-11-26T22:32:00+00:00
closed_at: 2026-11-26T22:32:00+00:00
---

# Description

A trusted consumer reports that every `wrkslots create ... --eden
main=<repo>` on its host refuses with rc 3: "cannot list Eden
checkouts in <the owner's default Eden state dir>: owner-checkout
guard REFUSED". No new Eden slot can be created there.

Cause: the `eden_host_cap` census (`eden.checkouts_by_state_dir`, fed
by `eden.discover_config_dirs` from `_eden_cap_report`) runs `eden
--config-dir <dir> list` for EVERY Eden state dir it finds: the
known list seeds the user's default instance (`~/local/.eden`), and
the `/proc` scan adds every running daemon's `--edenDir`, the default
daemon included. The census fails closed when one dir cannot be
queried. The consumer's owner-checkout guard refuses any eden call
against the owner's default instance, read-only `list` included,
because the owner's rule is that agents never touch that instance.
The guard stays; wrkslots must stop calling it.

A second, quieter call has the same problem: Eden removal measures
orphaned redirection bytes with `eden du --json <mount>` and no
`--config-dir`, which resolves through the default instance; a
refusal there is tolerated, so the orphan check silently weakens.

# Design

- The census queries only instances wrkslots itself manages: state
  dirs named `.eden-state` (a slot's own daemon dir, in this project
  or in another wrkslots project on the host, found by the `/proc`
  scan). It stays fail-closed for those: a managed dir that cannot be
  listed refuses creation, as today.
- The default instance (and any other instance wrkslots does not
  manage) is never invoked and none of its files are read. Unmanaged
  state dirs seen in daemon command lines are reported by path only.
- The default instance counts as a fixed reserve against
  `eden_host_cap`: its checkouts are assumed at a stated maximum
  (default 3, the owner's per-host cap for that instance), so the
  refusal fires when outer managed checkouts plus the reserve reach
  the cap. The refusal and status/doctor report name the reserve.
  Whether the reserve becomes a config key is a separate,
  persisted-config decision (old pinned readers refuse unknown keys).
- `eden du` in removal always passes the slot's own `--config-dir`.
- wrkslotsd does not mirror the census (it only validates config),
  so the Python edition carries the fix.

# Acceptance Criteria

- Unit tests with a fake runner that FAILS any eden call naming the
  default instance (or lacking `--config-dir`): the census, the cap
  refusal, status/doctor cap section and the remove `eden du` step
  never trip it.
- A managed `.eden-state` dir that cannot be listed still refuses.
- The cap refuses at outer managed checkouts + reserve >= cap, and the
  message names the reserve.
- Focused `test_eden.py` and strict mypy green; the gate green.

# Notes

Consumer-blocking: the newest Eden slot on the reporting host
predates the guard (2026-11-23).

Follow-up (not in this fix): make the reserve configurable with a new
`configuration` key (for example `eden_default_instance_reserve`,
integer, minimum 0). That is a persisted-config field, so it waits
until pinned older readers accept it; this fix keeps the reserve a
constant and adds no config key.

## Resolution

Landed as 2d9212a2 on main (gated 6c1f8f76, owner-tier gate pass,
replayed over unrelated main commits). The census queries only state
dirs named `.eden-state` (`eden.split_managed_config_dirs`), and
`eden.checkouts_by_state_dir` refuses an unmanaged dir before any eden
call; it stays fail-closed for managed dirs. The default instance is
never invoked or read: it counts as `eden.DEFAULT_INSTANCE_RESERVE`
(3) inside the report's `outer`, the refusal names the reserve, and
unmanaged dirs are listed in `unmanaged_state_dirs`. Removal's `eden
du` passes the slot's `--config-dir`. Tests run every stubbed eden
runner through a fake owner guard that fails any call naming the
default instance or lacking `--config-dir`. No config key was added
(see the follow-up above).
