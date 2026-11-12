---
title: 'gate-defer-merge-queue: defer gate starts while a host-configured unit pattern is active'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-12T11:22:18+00:00
updated_at: 2026-11-12T11:22:18+00:00
---

# Description

[watermelon-full-preview] This host donates CPU to another team's merge queue. Its validations run as user units, and synthetic or heavy load during one of them can eject a good change on CPU inflation. On 2026-11-11 one of our gates and a CPU-burner proof ran during such a validation; the other team reported it with per-cgroup data. Since then every lane checks `systemctl --user list-units --no-legend 'validate-merge-queue*' --state=active` by hand before gating. Both leads have approved automating that check in `scripts/gate.py`.

# Design

- `gate.py` defers a gate's start (the admitted command does not run, not merely its admission ticket) while either deferral signal is in force, and starts it when they clear. Signal 1: any user unit matching a host-configured pattern is active; the check is `systemctl --user list-units --no-legend '<pattern>' --state=active` resolved on PATH. Signal 2 (measurement holds): a host-configured directory holds `<name>.json` declarations, each naming a marker file (`marker_file`, or `status_file`; a relative path resolves against the host-configured `defer_holds_root` when set, else the holds directory) and a `max_hold_seconds` cap; the hold is in force from the last `WINDOW-START <UTC>` line of that marker file until a later `WINDOW-END` line, capped at `max_hold_seconds` after the start. `gate.py` reimplements this small file contract itself: it does not import the declaring team's library, and the declaring team's name and directory path never appear in the repository — the directory is host configuration (`defer_holds_dir` in the host file, `$GATE_DEFER_HOLDS_DIR` as override). Optional signal 3: a host-configured quiet command (`defer_quiet_command`, `$GATE_DEFER_QUIET_COMMAND`) whose exit 0 means OK to run; any other exit, or a command that cannot run, defers.
- The pattern is host configuration, never repository configuration: no new keys in the repo's `gate.toml`, and the other team's unit name is never hard-coded in the repo, so agent-utils stays neutral. Host configuration lives in a sibling per-host file, `~/.local/etc/agent-utils/gate-defer.toml` (override `$GATE_DEFER_CONFIG`), holding `defer_unit_pattern = "<glob>"` and optionally `owner_defers = true/false`, `defer_holds_dir = "<path>"`, and `defer_quiet_command = "<command>"`; the `GATE_DEFER_*` environment hooks are the test/override hooks and win per key. A sibling file (not a new key in `gate.toml` or `gate-slots.toml`) follows the phase-priority (#1146) and interleave (#1219) precedent: older checkouts refuse or cap on unknown keys in the shared files. An empty or unset pattern means no deferral.
- Waiting is poll-driven (`$GATE_DEFER_POLL_S`, default 1 s) with no wall-clock budget that can fail a gate (owner rule 346): deferral time does not count against `--wait-timeout`, and there is no deferral deadline.
- Measurement holds fail closed: a declaration or status file that cannot be parsed counts as an active hold, and `gate.py status` names the file and the problem. An empty (or absent) holds directory means no hold.
- `gate.py status` shows a deferred gate and what it waits on (the unit, the hold, or the quiet command); `gate.py queue --explain` names the same blocker. The waiting gate records the blockers on its ticket (`defer_waiting`), so a separate status process can show them.
- A gate already running is NOT paused, and this is deliberate: pausing (for example SIGSTOP) would freeze the validation scheduler mid-step, stall timers and file-system state the steps own, and can itself cause the timeouts the deferral exists to prevent. Deferral is start-only, exactly like blackout windows (#907): running gates are never touched.
- Owner-priority gates (`GATE_PRIORITY=owner`, #1180) defer too, unless the host file says otherwise with `owner_defers = false`. Choice: deferring is the default for every priority, because CPU inflation harms the protected validation regardless of who asked for the gate; a host that wants owner work to proceed says so explicitly in its own file.
- A malformed defer file refuses a start with a clear error (exit 2), like a malformed blackout file: the file exists only because the host asked for protection, so a typo must not silently remove it. If `systemctl` itself cannot be run, the gate warns and proceeds rather than wait forever on a broken probe.
- Fixture tests cover the hold contract: an active hold, an ended hold, an expired hold (past `max_hold_seconds`), an unparseable declaration (defers), and an empty directory (no deferral).
- Tests use a stubbed `systemctl` on PATH, never the real one, with bounded, paced churn: no unbounded spawn loops, and `/bin/true`, not a shell builtin, where a trivial command is needed.
- `gate.py --help` and the gate userguide text (the module docstring) describe the deferral, the host file, and the running-gate decision.

# Acceptance Criteria

- With a stubbed `systemctl` reporting an active unit matching the host pattern, a gate's command does not run while the unit is active and runs once the stub reports none; with an empty or unset pattern the gate runs immediately.
- `gate.py status` shows the deferred gate and the unit it waits on while it waits.
- An owner-priority gate defers under the default host configuration, and starts without deferring when the host file sets `owner_defers = false`.
- A gate already running when a matching unit becomes active is never paused or stopped by the deferral.
- The gate test suite passes with the stubbed systemctl; strict mypy passes on every changed Python file.
- Lands on a normal gate.

# Notes

Filed by mu-gatedefer, slot gatedefer, branch mu-gatedefer/gate-defer-merge-queue. Number reserved from devbig030's interim block (1400-1469).
