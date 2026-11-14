---
title: 'gate-defer-shared-measurement-holds: gate defer also reads the shared measurement-holds directory'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-14T00:00:00+00:00
updated_at: 2026-11-14T00:00:00+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1406 gate-defer-merge-queue and
#1489 gate-defer-patterns-and-observe. `scripts/gate.py` deferral
(`measurement_hold_blockers`, `DeferSettings`, `_defer_from_mapping`) reads
measurement holds only from one configured `defer_holds_dir` (a single
consumer's directory, marker-file contract with `WINDOW-END` lines). All
teams on the host agreed on 2026-11-14 to a SHARED hold directory,
`~/.local/state/measurement-holds/*.json`, schema:
`{"schema_version": 1, "marker_file": <path, usually the JSON itself>, "max_hold_seconds": N, "owner": "<team>/<agent>", "reason": "...", "started_at": "<UTC ISO>"}`.
A lane's gate must honour other teams' shared holds while not being held
by its own.

# Design

1. Shared directory: gate defer ALSO reads the shared directory, in
   addition to `defer_holds_dir`. Configurable host key
   `defer_shared_holds_dir` in the gate-defer host file (same keys accepted
   in the gate-slots fallback), defaulting to
   `~/.local/state/measurement-holds`; an empty string disables it.
   `$GATE_DEFER_SHARED_HOLDS_DIR` overrides per key (empty disables), like
   the other `GATE_DEFER_*` hooks.
2. Owner-aware exemption: a gate run whose declared measurement owner
   (`$GATE_MEASUREMENT_OWNER`, matched against the hold's `"owner"`) equals
   a hold's owner is NOT deferred by that hold, so a lane's own gate and
   `gate.py --rerun` load checks are not held by its own measurement hold.
   Other holds still defer it. Holds without an owner are never exempted
   this way.
3. Expired holds (`started_at + max_hold_seconds` in the past) never defer
   anything, and each expired hold is reported ONCE (a stderr line plus a
   row in the small report state file
   `~/.local/state/agent-utils/gate-defer-hold-reports.json`, override
   `$GATE_DEFER_HOLD_REPORT_STATE`) so its owner cleans it up. The same
   expiry rule applies to `defer_holds_dir` holds. A hold that ended with
   a `WINDOW-END` line is ended, not expired, and is not reported.
4. Malformed holds FAIL CLOSED (amended by the consumer's lead, replacing
   the original report-and-ignore rule): an unparseable JSON file, or one
   with missing or invalid fields, DEFERS and is reported, naming the file
   and the parse error. Reason: a malformed hold most likely means someone
   is mid-measurement and wrote the file badly, and ignoring it is what
   ruins a measurement. To avoid a silent permanent block it is reported
   again every hour it persists (injected clock in tests; no sleeps).
5. Missing marker: a missing `marker_file` counts as expired ONLY if the
   hold also has no future end (cap already passed / no `WINDOW-END` in
   the future). Otherwise it defers and is reported (hourly, like a
   malformed hold).
6. Status: `gate.py status` / defer status lines show shared holds
   (owner, reason, remaining time) and which were exempted or expired.
7. Docs: the gate defer section (module docstring and `--help`, the gate
   user guide) describes the shared dir, the schema, the exemption env
   var and expiry. No consumer names, host names or local paths other
   than the generic `~/.local/state` default in agent-utils files.
8. Tests: hermetic (tmp dirs, injected `now` via the evaluation functions'
   `now` argument and `$GATE_DEFER_NOW`), NO wall-clock sleeps or timing
   budgets (owner rule 346); cover active, exempt, expired-reported-once,
   malformed (fail-closed, hourly re-report), missing-marker both ways,
   and both dirs combined.

# Acceptance Criteria

- A gate defers on an active shared hold in the default-configured shared
  directory as well as on a `defer_holds_dir` hold; both dirs combined
  defer on either.
- A gate whose `$GATE_MEASUREMENT_OWNER` equals a shared hold's owner is
  not deferred by that hold and is still deferred by another team's hold.
- An expired hold (either directory) never defers and is reported once
  (stderr + report state file), not again on the next check.
- A malformed shared hold (unparseable, missing or invalid fields) defers
  (fail closed), is reported naming the file and the parse error, and is
  reported again after an hour persists.
- A missing marker file defers when the hold has a future end, and counts
  as expired (reported once, no defer) when its cap already passed.
- `gate.py status` shows shared holds with owner, reason and remaining
  time, and marks exempted and expired holds.
- Hermetic tests cover the above; the touched test module plus the
  ruff/mypy checks validation runs for `scripts/gate.py` pass locally.

# Notes

Follow-up to #1406 gate-defer-merge-queue and #1489
gate-defer-patterns-and-observe (`scripts/gate.py`). Number 1521 reserved
by the coordinating lane for this work; slug
gate-defer-shared-measurement-holds. The malformed-hold and
missing-marker rules above are the consumer lead's amendment of
2026-11-14, recorded here as agreed.
