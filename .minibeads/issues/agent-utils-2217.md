---
title: 'agentctl-lock-contention-no-argv: lock contention reports name processes by pid and agent, never by full command line'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- security
created_at: 2026-11-24T12:44:00-05:00
updated_at: 2026-11-24T12:44:00-05:00
---

# Description

When `agentctl send` is refused because another process holds the agent's lifecycle lock, the refusal JSON includes a contention report built by `lock_wait::contention`. That report carries `"command"`, the full command line from `/proc/PID/cmdline`, for the lock holder and for every waiting sender. Any caller that hits a busy lock therefore sees other processes' complete argv in its own tool output. The agent watcher's delivery alarms carry the same report.

Command lines can hold paths, file names of private briefs and, in other setups, credentials passed as arguments. Consumers have a standing rule never to print full process command lines, and this output breaks it without the caller asking. Seen by a consumer lane on 2026-11-24; no secrets appeared in that instance.

# Design

- `contention` reports each process as `pid`, `comm` (from `/proc/PID/comm`, 15 bytes) and, where it can be derived without exposing arguments, the agentctl subcommand and target agent name (for example `send liaison`). It never includes the raw command line.
- `command_of` / `command_line` stop being used for any user-visible report. A debug path that truly needs argv must be explicit and opt-in, and must not run by default.
- The same rule applies to every other report that names a foreign process: watch alarms, lock-wait messages, restore and revive refusals. Grep for `command_line(` callers that format into output, and convert them.

# Acceptance Criteria

- Test: a contention report for a held lock with a waiting sender contains pid and comm (plus subcommand and agent where derivable) for both, and no field holds the full command line. The test plants a sentinel argument and asserts it is absent from the JSON.
- Test: a watch delivery alarm built from a contention report contains no command line.
- An audit of `command_line(` callers is recorded in the resolution, with each output-facing one converted.

# Notes

Reported by a consumer team's coordinator on 2026-11-24. Interim mitigation on their side: lanes don't quote that output.
