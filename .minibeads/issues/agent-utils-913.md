---
title: 'unknown-harness-pins-foreground: --harness unknown pins the pane''s actual foreground program, not a process named unknown'
status: open
priority: 1
issue_type: bug
assignee: '014: mu-orc'
created_at: 2026-11-06T00:18:43.898584952+00:00
updated_at: 2026-11-06T00:18:43.898584952+00:00
---

# Description

With `--harness unknown`, agentctl pins a process literally named "unknown". `resolve_harness_executable` (client.rs, around line 3408) treats the harness KIND as the executable name, so adopt only pins a process whose executable basename is "unknown" (term.rs `harness_candidate` / `process_match_class`). Every real unknown TUI, such as orc or agentterm, therefore needs `AGENTCTL_UNKNOWN_BIN` set. The owner asked why, and wants it fixed independently of other work.

# Design

- For the unknown kind, adopt (and start, and repoint, where they pin) records and pins the pane's ACTUAL foreground executable. The pin is bound to identity: the executable path plus the process start time. No environment variable is needed.
- `AGENTCTL_UNKNOWN_BIN` stays as an optional override. When it is set, the foreground executable must match it, or adopt refuses with both names.
- Status, liveness and send checks use the recorded identity, so a different program later started in the pane is not mistaken for the adopted one.

# Acceptance Criteria

- Test: adopt an unknown TUI (a fake program with an arbitrary name) without `AGENTCTL_UNKNOWN_BIN`. The record pins that program's path and start time, and status reports it alive.
- Test: after the pinned program exits and another starts in the pane, the agent is reported dead or replaced, never alive.
- Test: with `AGENTCTL_UNKNOWN_BIN` set to a different name, adopt refuses and names both.
- Help and the userguide document that the unknown harness pins the actual foreground program.

# Notes

Owner request 168, relayed by the lead on 2026-11-05 at 7:57 PM ET (P1, small). A lane on the lead host found the root cause in a trial and will do the fix, starting once that host can create a slot again (waiting on the #898 recovery fix). It touches the same area as #852 D (unknown and agentterm pane evidence, cl-start), so coordinate there.
