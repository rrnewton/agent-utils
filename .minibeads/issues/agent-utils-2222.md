---
title: 'agentctl-watch-archived-agent-sweeps: watch sweeps never report an archived or stopped agent as unknown'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- watch
created_at: 2026-11-24T15:32:00-05:00
updated_at: 2026-11-24T15:32:00-05:00
---

# Description

After an agent was stopped cleanly (`agentctl stop`, exit 0, its pane and tab closed), its record moved to the registry's archive. But `agentctl watch` keeps reporting, every cycle, a problem it can't repair: "hooks_trust <name>: unknown agent". The watcher holds a stale entry for the archived agent and runs the Muse hooks-trust sweep (#1932) against a name that's no longer registered. The sweep then fails to load the record.

Seen on 2026-11-24 on a consumer team's registry, with a watcher on a build that already has #2121 (a missing record is silent). So this is a second path: the name comes from a stale list or observation, not from a directory scan.

# Design

- Every per-agent watch sweep (hooks trust, and any other sweep keyed by agent name) works from the CURRENT registered names, read at the start of the pass. An agent that's archived, or whose record is gone, is dropped from observations before any sweep runs.
- If an observation outlives its record between reading the list and the sweep (a race), the sweep treats "unknown agent" as "nothing to do", silent, the same as #2121's missing-record rule. A malformed existing record still reports.
- Find where the stale name comes from (a cached observation list, the watch state file, or the previous pass's report) and stop it carrying archived names forward.

# Acceptance Criteria

- Test: an agent observed in pass N and archived with `agentctl stop` before pass N+1 produces no hooks_trust or other per-agent problem in pass N+1, and isn't listed at all.
- Test: an observation whose record disappears mid-pass makes the sweep return nothing for it, silently, while a malformed record still reports.
- The watch state file, if it stores agent names, is pruned of archived agents on the next pass.

# Notes

Related: #2121 (a hooks directory is not an agent; a missing record is silent), #1932 (Muse hooks-trust sweep). Also seen the same day, separate and lower priority: the hygiene census reports "pane census budget reached; unprobed panes were kept, not closed" on most passes on busy registries.
