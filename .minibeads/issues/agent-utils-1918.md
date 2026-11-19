---
title: 'agentctl-watch-orphan-tab-race: a tab created during the pane census is closed as an orphan'
status: open
priority: 0
issue_type: bug
labels:
- agentctl
- watch
- herdr
created_at: 2026-11-19T16:38:00-05:00
updated_at: 2026-11-19T16:38:00-05:00
---

# Description

[watermelon-full-preview] The watch hygiene pass takes its pane census first (every pane's info, idle state and screen; with about 110 panes that took over 90 s), then reads the managed workspaces' tab order fresh from herdr. A tab created between the two reads is in the tab order but not in the census, so `tabs::plan` calls it an "orphan tab, no live pane" and emits CloseTab, gated only by `close_bare_shells`. Herdr closes the tab and SIGKILLs its processes, so any lane whose tab appears during a census window dies. Confirmed closed_bare_shells entries match lane deaths on 2026-11-19.

# Design

- Snapshot the tab order before the pane census, so a tab can be an orphan only if it existed before the census began.
- Track first-seen time per orphan tab like the bare-shell `bare_since` clock; a tab first seen this pass is never closed, and an orphan closes only past the bare-shell grace period.
- At close time, re-list that tab's panes from herdr and skip the close if any pane exists.
- Genuinely empty tabs that persist across passes past the grace period still close. Python edition has no hygiene logic; Rust only.

# Acceptance Criteria

- A tab created between the census and the order read is not closed.
- A tab first seen this pass is not closed.
- A tab whose re-list at close time shows a pane is not closed.
- A genuinely orphaned tab seen across two passes past the grace period is closed.
- Tests use a fake herdr with a controllable clock and pass sequence; condition-based waits only, no wall-clock budgets.

# Notes

Containment already live: both registries' watch config sets close_bare_shells=false; those files are not changed by this fix. New persisted field `orphan_tab_since` in the watch health report mirrors `bare_shell_since`; reported to the coordinator before gating.
