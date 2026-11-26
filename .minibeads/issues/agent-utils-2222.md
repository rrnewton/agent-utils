---
title: 'agentctl-watch-archived-agent-sweeps: watch sweeps never report an archived or stopped agent as unknown'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- watch
created_at: 2026-11-24T15:32:00-05:00
updated_at: 2026-11-26T07:45:00-05:00
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

## Resolution

Landed on main in the agentctl stack a724562b..7e89d795 (12 commits replayed by land-main from the gated 62a72cde, the same patch, in the order #2218, #2214, #2222, #2217), as 9118a57e. Gate ticket 1795671224491121398-107888b7: full selection, 65 steps, pass, finished 7:32 AM ET on 2026-11-26 (rust.agentctl 3106 passed, 0 failed; both python.agentctl shards ran, 1457 passed each).

- The stale name came from the observation list carried between passes. `watch_agents_once` now reads the current registered names once at the start of each pass and drops observations for unregistered names before any per-agent sweep runs. The same listing prunes the watch-state memory and pending notices.
- The hooks-trust sweep answers through `answer_hooks_trust_if_dialog` and treats "nothing to answer" as silent. That call returns nothing when the record vanishes between the listing and either load (a stop archiving it mid-pass), while a malformed existing record still errors.
- Tests: `an_agent_archived_between_passes_is_dropped_from_every_sweep_and_from_state`, `a_hooks_record_that_vanishes_mid_pass_is_silent`, `an_archived_muse_agent_is_silent_to_the_hooks_sweep`.
- No persisted-field changes. The pane-census budget message noted above is separate and not addressed here.
