---
title: 'agentctl-load-robust: load-flaky agentctl cases are found case by case and quarantined until robust'
status: open
priority: 1
issue_type: task
assignee: mu-testaudit
labels:
- agentctl
- test-robustness
created_at: 2026-11-10T22:46:59+00:00
updated_at: 2026-11-10T22:46:59+00:00
---

# Description

[muse watermelon] Owner, reply 346 (2026-11-10): "Disable those tests and have an agent work to bring them back when they're robust. Each product lane must make its tests fast, high signal, and robust to running on a hot machine (>75% cpu on 316 HW threads). ... Anything else should be disabled until we can improve it."

`python.agentctl.shard-1` failed 10 times in the last 3 days of the gate ledger, every time on a head that later passed a solo rerun; `rust.agentctl.test` failed 6 times, 4 later rerun-passed. Shard-level statistics do not name cases, and the agentctl suites drive real processes, sockets and terminal backends where load sensitivity hides in individual cases. Coordinator decision: case-level hunt first (gate logs for failed case names, then the timing mechanism in each case body), then quarantine named cases only, by the same mechanism as #1267 wrkslots-load-robust. No whole-shard or whole-crate disable without coordinator approval.

Out of scope: the `herdr-agent` and `agentctl` cross-language differentials (the five gate-exclusive steps) are owned by #1249 cross-differentials-robust and are already disabled by default; this issue covers only the Python and Rust unit and integration suites.

# Design

- Hunt: extract failed case names for `python.agentctl.shard-*` and `rust.agentctl.test` from gate logs of the last 3 days; for each repeat offender, open the case body and quote the timing mechanism (sleep or poll racing a real process, deadline or budget assertion, fixed port or path collision). Cases without a timing mechanism are real failures: file them as product bugs, never quarantine them.
- Quarantine: same marker and opt-in as #1267 wrkslots-load-robust; each quarantined case is listed here with its mechanism and return condition (event or condition waits; no verdict decided by elapsed wall or CPU time on a loaded host).
- Proof bar for return: repeated passes under load on this host class (316 hardware threads, >75% CPU), recorded in Notes; touched-code flake rule applies.

# Acceptance Criteria

- Every repeat gate-failing agentctl case of the audit window is dispositioned in Notes: quarantined with mechanism, filed as a product bug, or shown to be a one-off with evidence.
- Quarantined cases are excluded by default, included by the opt-in, and named as quarantined under this issue.
- Shards and the Rust suite otherwise run their full case sets.

# Notes

Filed by the test-robustness audit lane (mu-testaudit). Related: #1267 wrkslots-load-robust (same quarantine mechanism), #1249 cross-differentials-robust (differentials, separate).
- Hunt (mu-testaudit, 2026-11-10): my four gate logs name no failing agentctl case; `rust.agentctl.test` failed once (389s, exit 1) with no retained case detail. No case quarantined without a name and mechanism; next red must retain case names from the step detail.
