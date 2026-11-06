---
title: 'codex-ready-detection: Codex status never goes idle, so revive waits out its startup timeout'
status: open
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-06T03:45:00.000000000+00:00
updated_at: 2026-11-06T03:45:00.000000000+00:00
---

# Description

During a revive benchmark, a Codex agent's status never went idle. `agentctl revive` waited about 184 s against `--startup-timeout 180`, though the session answered a prompt in 1.3 s. That makes Codex revive latency unmeasurable, and it probably accounts for the Codex "incomplete" in #920.

# Design

Update Codex idle/ready detection to match its current TUI. Find the idle markers it shows today (prompt row, footer, status line) and recognise them, keeping the older markers for older builds. If readiness can't be decided from the screen, say so in status instead of waiting out the timeout silently.

# Acceptance Criteria

- A test with current Codex TUI screen captures (idle, working, and starting up) classifies each correctly.
- Revive of a Codex agent returns once the session is ready, well under the startup timeout, in a test with a fake or recorded Codex screen.
- Existing Claude and Muse readiness tests are unchanged.

# Notes

Reported 2026-11-05 10:40 PM ET by the lead coordinator, from a consumer's revive benchmark.
