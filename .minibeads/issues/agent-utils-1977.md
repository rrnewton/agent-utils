---
title: 'eden-daemon-restart: restart per-slot Eden daemons after a reboot as each teams own restore step'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- eden
- restore
created_at: 2026-11-20T13:51:20-05:00
updated_at: 2026-11-20T13:51:20-05:00
---

# Description

[watermelon-full-preview] From the 2026-11-20 reboot drill: per-slot Eden daemons (state dir `<slot>/.eden-state`) are restarted by nothing after a reboot, so only 1 of a team's 23 Eden checkouts came back usable. wrkslots has no Eden restart verb: a daemon starts implicitly at `eden clone` during slot creation and stops only on slot removal, and a dead daemon surfaces later as status/finish refusals. Each team must be able to restart the daemons for its own slots during its own restore; no step may start daemons for another team's registry.

# Design

Lane mu-restoreboot, filed with #1975 restore-boot-recovery and landing separately.

(1) A wrkslots verb (working name `eden restart-slot-daemons`, Python edition; Eden has no Rust mirror) iterates this project's active records with an Eden checkout and an existing `<slot>/.eden-state`, queries `eden --config-dir <slot>/.eden-state info` per slot, skips healthy daemons, restarts dead ones, and re-verifies with `info`. Discovery stays scoped to the project's own configured worktree roots, as `_eden_known_config_dirs` already is; another project's slots are never enumerated. The existing host cap (`eden_host_cap`) accounting stays fail-closed: a daemon that cannot be queried counts against the cap decision exactly as at creation.
(2) agentctl restore calls the verb as its own team's step (after the slot-image remount, before slot holds), limited to slots bound to agents in its own registry, and records per-slot outcome rows in the restore report and the per-boot log. A team that does not configure the step gets nothing. The step is idempotent: a second restore of the same boot finds every daemon healthy and starts none.
(3) No idle reaping and no changes to daemon lifetime outside the restart: daemons still stop only on slot removal.

# Acceptance Criteria

- With a fake `eden` binary: a slot whose `info` reports dead is restarted and re-verified; an already-healthy daemon is a no-op (no start invocation recorded); two consecutive runs start each daemon at most once.
- A `.eden-state` under another project's worktree root is never queried or started, and agentctl restore only restarts slots bound in its own registry (scratch-registry test).
- When the host cap would be exceeded or a daemon cannot be queried, the verb refuses that slot with a named reason and continues the rest; the restore report carries the refusal row.
- Restore completes (with refused rows) when the `eden` binary is absent or fails; the step never blocks lane revives.
- New test files are classified in validation/components.json; the landing passes the gate.

# Notes

Related: #1975 restore-boot-recovery (same lane; this issue lands separately), #1804 wrkslots-eden-multiple-backing-repos (closed; per-slot daemon layout), #1730 (Eden mounts outside the project). Persisted fields: none — only the Eden daemon's own state under `<slot>/.eden-state` is touched; restore report JSON gains additive rows.

Recorded limitation: the verb takes no project lock against a concurrent slot remove, so a remove racing a restart can resurrect a daemon for a slot being deleted. Closing that race needs the remove path and the restart verb to share a lock; it is noted here rather than fixed in this change.
