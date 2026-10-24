---
title: 'revive-stat-parse-flake: a revive dry run can be blocked by an unparsable process stat in the cross differential'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- flake
- validation
created_at: 2026-10-24T12:38:03.782271146+00:00
updated_at: 2026-10-24T12:38:03.782271146+00:00
---

# Description

[opus 5.5] Found by #494 test-time-agent-utils on 2026-10-24, in a full-graph gate on the loaded validation host: cross.agentctl.differential failed with one divergence out of 760, primary/revive/rust-to-python/ready/dead-dry-run/plan: one edition's revive dry run returned action "blocked" with reason "cannot parse process stat for pid N" instead of a plan. That message comes only from parse_process_stat in rs/agentctl/src/client.rs, which refuses a /proc/<pid>/stat read that is empty or truncated, as a read of a process exiting at that moment can be. The commits under test did not touch the cross harness or agentctl's runtime code, and the step passed in the previous gate of the same tree and the baseline gate. Log kept at /tmp/cl-topt-au/gate3-revive-flake.log on the validation host. Suspect: a revive harness process the fixture just retired is read between its exit and its reaping; the Rust edition reports a parse failure where the Python edition (or a retry) sees an exited process.

# Acceptance Criteria

A process that exits while its stat is read is classified the same way by both editions (exited, not unparsable), with a test that reads a stat of an exiting process; the revive dry-run case passes repeatedly under load.
