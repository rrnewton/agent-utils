---
title: 'agentctl-services-refresh-reload-safe: services refresh --no-reload never runs a user-manager daemon-reload, refuses a unit-file move that needs one, and a link-form drop-in makes later moves reload-free'
status: closed
priority: 1
issue_type: feature
assignee: cl-svcreload
labels:
- agentctl
- services
- hot-machine
created_at: 2026-11-27T23:55:00-05:00
updated_at: 2026-11-28T00:31:00-05:00
closed_at: 2026-11-28T00:31:00-05:00
---

# Description

`agentctl services refresh` moves a unit-file service onto a new snapshot by writing the drop-in `UNIT.d/zz-agentctl-services.conf`, running `systemctl --user daemon-reload`, then restarting the unit. Its rollback reloads too. A consumer team forbids any user-manager reload while their transient units run: on 2026-11-23 a reload made systemd report a live transient scope as dead, and their stop-by-name meters went blind. So that team cannot refresh its watch daemon, a unit-file service whose drop-in this tool wrote, while their work runs.

A plain "write the drop-in, restart, skip the reload" does not work. systemd applies a changed drop-in to a loaded unit only after a daemon-reload. systemd.unit(5) says so ("Use systemctl daemon-reload or an equivalent command to reload unit configuration while the unit is already loaded"). An active service is always loaded. After the write, systemd reports `NeedDaemonReload=yes` for the unit, and systemctl warns "The unit file, source configuration file or drop-ins of UNIT changed on disk". A restart without the reload runs the old `ExecStart=`. The health check would then fail on the old binary and roll back, or a later reload by anyone would start the new binary with no health check.

Transient units are not affected: they are moved with stop and `systemd-run`, which never reloads.

# Design

Part 1, `services refresh --no-reload` (also passed through by `services turnover --no-reload`):

- No path calls `daemon-reload`: not the change, not the rollback, not the inactive-oneshot update.
- Transient units are moved as today (stop plus `systemd-run` replay).
- For a unit-file service, the decision is made before anything is written. The unit is refused, unchanged, with a reason that starts `needs-daemon-reload:`, when either:
  1. the drop-in bytes the plan would write differ from the file on disk (or no file exists), since writing them sets `NeedDaemonReload=yes`; or
  2. `systemctl --user show -p NeedDaemonReload UNIT` already says `yes`, so what is on disk is not what is loaded, and a restart would not run what the plan checked.
- A refused unit gets no write and no file timestamp change. It counts as a refusal (exit 1) and the next units are still tried.
- Otherwise the unit is restarted by its exact name and health-checked as today. After the restart, `NeedDaemonReload` must still say `no`, or the unit is rolled back by the same reload-free path.
- `--dry-run` shows the same refusals (action `refused`, text `refused (dry run): needs-daemon-reload: ...`, as for other plan refusals) and, per unit, `form` and `reload`.

Part 2, link-form drop-ins, which make Part 1 useful for unit-file services:

- The drop-in names `ROOT/links/UNIT/agentctl` in place of the snapshot path. For a bridge it also sets `AGENTCTL_HOME=ROOT/links/UNIT/home`. `ROOT/links/UNIT` is a symbolic link to the snapshot directory, and `ROOT` is the services state root that also holds `snapshots/` and `refreshes/`. The drop-in text never names a commit, so its bytes are the same for every snapshot.
- Moving a link-form unit means swapping the link atomically (a new link, then rename over the old one), then a restart and the health check. Nothing is written under the unit directories, and no reload runs, with or without `--no-reload`. Rollback swaps the link back and restarts.
- Migration: a refresh without `--no-reload` writes the drop-in in link form and runs the one reload. Operators run it in a window their reload rules allow. After that, every move of the unit is reload-free.
- `services prune` counts every snapshot a link points at as in use. Discovery and the report resolve the link and show the snapshot it points at.

Report and rollback record:

- Each report row gets `form` (`transient`, `drop-in`, `link`) and `reload` (`none` or `daemon-reload`: what the move ran).
- `ROLLBACK.sh` blocks for link-form and reload-free moves have no `daemon-reload` line. They name the previous link target, which is also saved beside `UNIT.before` as `UNIT.previous-link`.

Part 2 is a simpler form of the stable launcher in #2221. If it lands here, #2221 narrows to whatever the link form leaves out.

With Part 2, a move without `--no-reload` also reloads only when the drop-in must change or `NeedDaemonReload=yes`; the rest of its moves of a link-form unit are reload-free too.

Part 3, a read-only pre/post check for the one migration reload (asked for by the consumer team's lead): `agentctl services transients` lists every loaded transient scope and service with systemd's `ActiveState`/`SubState`, `ControlGroup` and `MainPID`, and from its cgroup `populated`, the processes in `cgroup.procs`, `cpu.stat` `usage_usec` and `memory.current`. `--save FILE` records the list (refusing an existing file); `--check FILE` reads both views again: a recorded unit whose cgroup still has processes is LOST when systemd no longer lists it, reports it inactive or failed, names another cgroup, or its CPU accounting stops reading or goes backwards; one whose cgroup is empty ENDED. Exit 1 when a unit was lost.

# Acceptance Criteria

- Test (fake systemctl): with `--no-reload`, a unit-file service whose drop-in would change is refused before any write. The drop-in file and its timestamp are unchanged, and the fake never sees `daemon-reload`, `restart` or `stop` for it.
- Test: with `--no-reload`, a unit whose `NeedDaemonReload` is `yes` is refused before any change.
- Test: with `--no-reload`, a transient unit is still moved by stop plus `systemd-run`, with no reload.
- Test: a link-form unit moves by swapping the link plus a restart, with no reload in either mode. A failed health check swaps the link back and restarts.
- Test: a refresh without `--no-reload` migrates a legacy drop-in to link form with exactly one reload.
- Test: prune keeps a snapshot a link points at.
- Test: the dry run and the JSON report show `form`, `reload` and the `needs-daemon-reload` refusals.
- Test: `services transients` keeps only transient units, and a check after a simulated reload names a scope whose cgroup still runs but which systemd calls dead (LOST), one without CPU accounting (LOST), one that ended, and new units.
- Test through the real CLI with a fake `--systemctl` script: `services refresh --no-reload` refuses a unit-file move with only read calls reaching systemctl and nothing written; `services transients --save` then `--check` exits 1 on a lost scope.
- Docs: `services refresh --help` and the services section of the agentctl user guide describe `--no-reload`, the refusal conditions, the link form and the one-time migration.

# Notes

Requested by the lead coordinator on 2026-11-27 for a consumer team whose standing rule since 2026-11-23 forbids user-manager reloads while their transient units run. Evidence for the reload requirement was read on systemd 262: systemd.unit(5) and the warning text in systemctl. No test touches a live user manager; all tests use the fake host or a fake `--systemctl`.

Conditions from the consumer team's lead for its migration (operational, after landing): (1) its pinned agentctl moves to a build with this change before any of its units is moved to link form, because an older `services prune` does not see a link as in use; (2) the one migration reload runs in a quiet window it names, with `services transients --save` just before and `--check` just after, its own meter names listed beside the unit names.

## Resolution

Implemented as designed in three parts: `--no-reload` on `services refresh` and `services turnover`, link-form drop-ins (`ROOT/links/UNIT`), and `services transients --save/--check`. The report rows carry `form`, `reload` and `link`; `ROLLBACK.sh` has no reload line for a reload-free move and saves `UNIT.previous-link`. `services prune` and `services list` resolve links. The guide's services section has the migration runbook.
