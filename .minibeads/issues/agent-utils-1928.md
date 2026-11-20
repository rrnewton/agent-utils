---
title: 'agentctl-permission-prompt-auto-deny: a lane launched before a profile gained its permission settings keeps the old argv on revive, so the settings never reach it'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-19T00:00:00+00:00
updated_at: 2026-11-19T00:00:00+00:00
---

# Description

[watermelon-full-preview] Owner ask 2026-11-18, reported by a consumer
team 2026-11-19: a Claude Code permission auto-deny configured through
a launch profile's `--settings` argument was deployed at 11:45 AM ET
on 2026-11-18, yet its log has never recorded a denial, and a
permission prompt from an in-process subagent (a Bash dialog for
`rm -f` on a variable path, the built-in dangerous-rm check) stopped
a lane at 6:14 PM ET on 2026-11-19.

Investigation (read-only toward the consumer) found that the affected
lane was created on 2026-11-17, before the profile gained
`--settings`; its live process carries no `--settings` flag
(presence-only check), and its stored launch spec has none either.
Lanes started after the deployment do carry the flag, so the start
path honours the current profile. agentctl's own watch policy never
answers a prompt by design, and the headless approval surface
(#671 headless-approval-surface, owner answer or a 600 s timeout
then deny) applies only to headless agents, not interactive lanes.

# Design

- Every launch path that relaunches a managed lane — start, a start
  with a native resume, revive, restart and restore — must make the
  profile's current permission-relevant arguments effective, or must
  refuse or warn by name when it replays a stale launch spec, so a
  profile change of this kind cannot silently never arrive.
- A permission prompt in a managed interactive lane is then denied
  by the harness mechanism the profile configures, and the denial is
  logged where that mechanism logs.
- No new persisted field is introduced without reporting it first
  (reader skew).

# Acceptance Criteria

- A lane whose stored launch spec predates a profile change is not
  revived, restarted or restored onto the stale arguments silently:
  it either launches with the current profile arguments or the
  operation names the drift and the command that fixes it.
- Tests cover every recognised harness prompt shape using fake panes
  and recorded prompt texts only (no keys in a live consumer pane),
  and the harness support matrix is updated in the same commit.
- Focused Python and Rust suites pass on the gated tree.

# Notes

Filed by the investigating lane before coding, pending the
coordinator's decision on whether revive should auto-refresh from
the profile or refuse on drift. The plugin-hook trust prompt is out
of scope: the owner reserves that decision and no tool may answer it.
