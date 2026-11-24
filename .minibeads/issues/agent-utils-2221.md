---
title: 'agentctl-services-refresh-no-reload: move unit-file services onto a new snapshot without a user-manager daemon-reload'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- services
- hot-machine
created_at: 2026-11-24T15:11:00-05:00
updated_at: 2026-11-24T15:11:00-05:00
---

# Description

`agentctl services refresh` (and the scheduled turnover built on it) moves a unit-file service onto a new snapshot by writing a drop-in that replaces `ExecStart=`, running `systemctl --user daemon-reload`, then restarting the unit. A user-manager reload is host-wide. On a shared host, a consumer team has seen a reload during metered runs make systemd report a live scope as dead, which blinded their stop-by-name meters. So that team forbids reloads while their boxes run. Their long post-land validations also run as user services, whose supervisors read unit state.

Result: a refresh of any unit-file service (headless wrappers, watchers, bridges) must wait for a window with nothing running. On a busy host that window may not come for hours, so services stay on stale builds. The daily turnover has the same problem: it needs a quiet-host gate, and still reloads.

# Design

- A stable launcher. The unit's `ExecStart=` names a fixed path, for example `$AGENTCTL_HOME/services/launch/<unit>`, written once, that reads the current snapshot id from a small state file next to it at every start and execs that snapshot's binary with the recorded arguments and environment.
- Moving a service onto a new snapshot then means: write the state file atomically (temp file plus rename), then `systemctl --user restart <unit>` (or try-restart). No drop-in change and no daemon-reload. Rollback is the same operation with the old snapshot id.
- A one-time migration converts an existing unit to the launcher form. That migration is the only step that needs one reload, and it says so, so operators can do it in a quiet window. After it, every refresh is reload-free.
- `services refresh --dry-run` shows, per unit, whether the refresh is reload-free (launcher form) or needs a reload (legacy drop-in form), so operators can see it before acting.
- The scheduled turnover uses the reload-free path for launcher-form units and keeps its quiet-host gate only for units still in legacy form.
- Health check and automatic rollback stay as they are today.

# Acceptance Criteria

- Test (fake systemctl): refreshing a launcher-form unit writes the state file and restarts the unit, and never calls daemon-reload. Rollback restores the previous id.
- Test: the launcher execs the binary named by the state file with the recorded argv and environment, and refuses with a clear error when the id names a missing snapshot. It never falls back silently.
- Test: a dry run reports reload-free against reload-needed per unit. The migration of a legacy unit is the only path that reloads.
- Docs: the services section of the agentctl user guide describes the launcher form and the one-time migration.

# Notes

Requested by a consumer team's lead on 2026-11-24: their headless wrapper can't be refreshed while their validations run, because the refresh plan includes a daemon-reload.
