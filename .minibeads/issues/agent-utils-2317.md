---
title: 'noresult-consecutive-alarm-and-incomplete-suite-visible: a step that ends NO_RESULT in consecutive gates raises a health alert, and a gate row whose suite did not complete says so'
status: open
priority: 1
issue_type: feature
assignee: cl-shardcap
labels:
- gate
- dagrun
- validation
created_at: 2026-11-26T17:56:00-05:00
updated_at: 2026-11-26T17:56:00-05:00
---

# Description

`python.agentctl.shard-0` and `python.agentctl.shard-1` ended NO_RESULT (OOM-killed at their learned memory cap) in every full gate from 2026-11-20 to 2026-11-26, and nobody saw it for six and a half days (#2306). A learned-cap NO_RESULT is non-failing by design (#1763, owner rule 346), so the step leaves `failed_steps` empty, the gate verdict degrades only to `partial`, and the ledger row looks like any other partial gate. A count of red steps over the last week cannot find it either: the ledger has no per-step NO_RESULT outcome to count. About 2900 agentctl Python tests went unmeasured while agentctl changes landed, and real failures sat on main unseen (#2253).

#2207 makes such a run exit 3, which stops a single gate from reading as green. Two things are still missing: a signal when the same step keeps ending NO_RESULT across gates, and a row that tells the reader, at landing time, that a selected suite did not complete.

# Design

- The gate ledger row records each selected step that ended NO_RESULT, by name, with the reason class (learned-cap OOM, other kill) and, where the runner reports it, the tests started and completed. A row with any such step carries a plain flag, `suite_incomplete: true`, beside its verdict.
- `gate stats` (or a new `gate health` subcommand) reads the ledger and reports every step whose two or more most recent runs, across distinct gates, all ended NO_RESULT, newest first, with the gate ids and heads. It prints one machine-readable `HEALTH` line per such step, in the tick-hub line format, so an operator's periodic health tick can raise it as an alert with no parsing of its own.
- Landing tools that read the ledger to accept a gated commit print "suite did not complete: <steps>" for a row with `suite_incomplete: true`, next to the verdict, so the person landing sees which suites never ran, and refuse it the same way they refuse a red step unless the operator passes an explicit waiver reason.
- No semantic change to NO_RESULT itself: a learned-cap breach stays non-failing for the step (#1763); only its visibility changes.

# Acceptance Criteria

- Test: a gate whose only non-green steps are learned-cap NO_RESULT writes a ledger row naming those steps and `suite_incomplete: true`.
- Test: given ledger rows in which a step ended NO_RESULT in its two most recent gates, the health report prints one `HEALTH` line for that step naming both gates; one NO_RESULT followed by a completed run prints nothing.
- Test: the landing check prints "suite did not complete" for such a row and refuses it without an explicit waiver reason.
- No wall-clock pass/fail budget is introduced (owner rule 346).

# Notes

Filed while fixing #2306 (agentctl-python-shards-learned-cap-oom), whose two shards ended NO_RESULT in 97 recorded runs before anyone looked. Related: #1763 (learned-cap NO_RESULT semantics), #2207 (a learned-cap NO_RESULT run exits 3), #2253 (latent agentctl failures hidden by the same NO_RESULT runs).
