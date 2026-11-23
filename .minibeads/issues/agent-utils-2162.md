---
title: 'agentctl-revive-tests-read-real-host-processes: two subagents revive tests fail on a busy host because they census real same-UID processes'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- flaky-test
created_at: 2026-11-23T10:15:00-05:00
updated_at: 2026-11-23T10:15:00-05:00
---

# Description

Two agentctl subagents revive tests fail solo on clean main on a busy shared host: a_muse_lane_holding_only_its_session_directory_records_it_at_start_and_revives and revive_requires_positive_missing_target_and_absent_unambiguous_census. They panic on errors such as "cannot inspect environ of same-uid process <pid> while proving no live holder ... refusing (fail closed)": the code under test runs a real census of the host's same-UID processes, and a live agent process belonging to someone else (unreadable environ, or a matching session) makes the census refuse. rust.agentctl.test is therefore red in gates on that host regardless of the change.

# Design

Make the tests hermetic: inject the process inventory (or point the census at a fake proc root) so the tests see only their own fixture processes; keep one opt-in integration test that uses the real host census, outside the gating set. Check the other revive and census tests for the same pattern.

# Acceptance Criteria

- Both tests pass 20 times in a row on a busy host with live agent processes of the same user, and still fail if the code stops refusing on a real holder in the fixture.

# Notes

Found while triaging gates; filed by the coordinator.
