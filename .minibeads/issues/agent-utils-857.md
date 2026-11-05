---
title: 'fixture-outlives-kill: test_launch_bridge_killed_first''s chat run must die with its test'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-05T00:25:31.112006848+00:00
updated_at: 2026-11-05T01:50:37.654397380+00:00
closed_at: 2026-11-05T01:50:37.654397249+00:00
---

# Description

A leaked test fixture outlived its test run by 34 hours. `test_launch_bridge_killed_first` started a release `agentctl ... chat run`, with its cwd in a pytest temp directory. When the outer gate step (python.agentctl.shard-1) was killed, that process was reparented to init and kept running. It held a dagrun scope busy, which blocked another team's quiet-host CPU profile, and it ignored SIGTERM for over 10 s, so it needed SIGKILL.

# Design

- The fixture's child must not outlive an outer kill. Run it in its own process group or under a subreaper, set PR_SET_PDEATHSIG on it so it dies with its parent, or kill the cgroup on teardown, and kill the whole group in a `finally`.
- Audit the other fixtures that start long-running agentctl services (`chat run`, `watch-agents`, `serve`) for the same pattern.
- Find out why `chat run` ignored SIGTERM for over 10 s (it should exit promptly on TERM), and fix it or explain it.

# Acceptance Criteria

- A test kills the outer pytest process (SIGKILL) during the fixture and asserts that no `chat run` (or other fixture child) remains.
- Gate passes.

# Notes

P1, reported 2026-11-04 by the other team via the lead; the coordinator stopped the leaked PID by hand.
