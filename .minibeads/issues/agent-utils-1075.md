---
title: 'agentctl-goal-wait-unbounded: agentctl goal can wait forever, ignore SIGTERM and outlive its target'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T17:30:00.000000000+00:00
updated_at: 2026-11-08T17:30:00.000000000+00:00
---

# Description

A consumer reported (2026-11-08) an `agentctl goal <agent> --file F` process, started 2026-11-07 about 08:40Z, that slept in `pause()` (wchan `__ia32_sys_pause`) for 32 h 43 min, was re-parented to the user systemd, and ignored SIGTERM; it needed SIGKILL. Its target agent had since been archived (`unknown agent`). The binary was the shared `~/.local/bin/agentctl`, which at that time was the services snapshot built from commit f8b834e5 (per the host's global-agentctl history), so later fixes may not apply; check whether current main still waits this way.

# Design

- Goal waits are bounded: a default timeout and an explicit deadline flag, documented in `--help`.
- While waiting, the command notices the target agent being stopped or archived (or its record disappearing) and exits with a clear error naming what happened.
- SIGTERM (and SIGINT, SIGHUP) end the wait promptly; nothing blocks all signals across the wait.
- If the wait parks in `pause()` or a signal wait, make sure its wake-up sources include the target's state change and the deadline.

# Acceptance Criteria

- A test: the target agent is archived mid-wait and `goal` exits promptly with the clear error.
- A test: the deadline passes and `goal` exits with a timeout error.
- A test: SIGTERM during the wait ends the process within a second or two.
- `--help` documents the default timeout and the deadline flag.

# Notes

Reported by a consumer's coordinator through the lead coordinator. Related: #1061 (another long-lived process ignoring SIGTERM).
