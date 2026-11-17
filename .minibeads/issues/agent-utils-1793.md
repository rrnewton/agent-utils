---
title: 'agentctl-workspace-cultivation: fully cultivated workspaces — duplicates, orphans, labels, headless viewers, lifecycle truth'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-17T18:45:00+00:00
updated_at: 2026-11-17T18:45:00+00:00
---

# Description

[watermelon-full-preview] Owner request (GitHub #345 tab-management), 2026-11-17: workspaces managed by agentctl must be fully cultivated — ordered as declared, tabs reflecting live agents with no duplicates, and empty shells closed. Follow-up (owner, same day): headless agents must ALWAYS show in a Herdr tab. New issue rather than an extension of #1225 workspace-goal-reconciler: #1225 owns the viewer goal/reconciler; this issue owns duplicate/orphan/lifecycle hygiene, the `agentctl tabs` listing, and wiring headless viewer creation into every creation path. The two cross-link; the watch healer here implements the viewer-recreation part of the #1225 goal for headless agents.

## What exists today (mapped from GitHub #345)

- Canonical tab order in managed workspaces and closing idle bare shells: `hygiene_pass` in `rs/agentctl/src/restore.rs` (since #979), `bare_shell_grace_seconds` 600, `close_bare_shells`. Managed means: the workspace holds a claimed pane or a named viewer tab.
- A headless record can name a viewer tab (`viewer_tab_id`, `agentctl set --viewer-tab`, `tail --follow` records its own tab) and hygiene ranks it with the agent (#1225 Phase 1).
- Tab label restore exists on repair paths (`doctor --repair-labels` equivalents in the Rust edition).

## Gaps

1. A workspace holding only duplicate or orphan tabs (no claimed pane) is outside the managed set, so hygiene never reorders it and never closes its bare shells. Live proof: duplicate-labelled bare shells survived indefinitely in a second workspace of the same registry.
2. No duplicate detection: claims key on pane id, so a second tab carrying a live agent's name is invisible.
3. No orphan-tab close: a tab whose pane is gone is never closed.
4. No label sync in the pass: a claimed tab whose label differs from its agent's name keeps the stale label (rename/revive/restore included).
5. No missing-viewer recreation: a headless agent with no viewer tab, a closed viewer tab, or a dead viewer pane keeps no view (#1225 later phases).
6. Headless creation does not create a viewer: `start --mode headless`, `run-headless`, and `install-units --headless` register the agent but open no tab.
7. Record lifecycle can be permanently wrong: a launch that errors after its harness process is up stamps `launch_failed` forever, even while the recorded process is the live foreground process of its pane. Nothing promotes it back to `running` on proven liveness.
8. No listing: no `agentctl tabs` showing each tab, its owner record, foreground process, idle time, and whether it would be closed; no dry run short of reading watch internals.
9. Canonical order must place the designated coordinator's viewer second, immediately after the liaison (owner ruling, 2026-11-17): liaison first, coordinator viewer second, then lanes.

## Work

- Managed-set expansion: a workspace is managed when it holds a claimed pane, a named viewer tab, or a pane whose tab label names a registered agent (a duplicate). Hygiene then reorders it and may close its bare shells under the existing grace.
- Duplicate resolution: when several tabs carry one agent's name, keep the tab whose pane is the registered live agent's pane; close another tab only when its pane is a proven idle shell with an empty composer, past grace. Never close a pane with a foreground process other than its shell, composer text, or a registered live agent.
- Orphan tabs (listed by the workspace tab order, holding no live pane) are closed by the pass.
- Label sync: a claimed or viewer tab whose label differs from its agent's name is renamed in the pass; rename/revive/restore keep the same sync.
- Headless viewers (first slice): creating a headless agent (`start --mode headless`, `run-headless`, `install-units --headless`) creates its viewer tab (console where usable, else `tail --follow`) in the managed workspace and records it as `viewer_tab_id`. Goal state: one viewer tab per live headless agent, in canonical order (liaison first, designated coordinator's viewer second). The watch healer recreates a viewer tab that was closed or whose pane died, in its goal position.
- Lifecycle truth: a `launch_failed` record whose pinned process is proven live as its pane's foreground process is promoted to `running` by the healer; promotion on liveness proof only, never on label or tab presence alone.
- `agentctl tabs` lists every tab of every managed workspace with owner record, foreground process, idle time, and would-close verdict; `--dry-run` prints the actions a pass would take and takes none. The layout-2 sentinel file at `agents/agent.json` must never be read as an agent (pinned by a regression test; readers already filter on `agents/<name>/agent.json`).

## Safety

Fake-based tests prove a pass never closes: a pane with a foreground process other than a shell; a pane whose composer holds unsent text (an unreadable screen counts as holding text); a pane a registry record points to; a live agent's viewer pane. Closable: an unregistered idle shell past grace, a duplicate idle shell past grace, an orphan tab with no pane. No wall-clock budgets in tests: grace is driven by injected time.

# Acceptance Criteria

- A pass over the live-proof shape (duplicate bare shells in a second workspace, one orphan tab) closes exactly those tabs and nothing in the workspace holding the live panes.
- `agentctl tabs` and `agentctl tabs --dry-run` list owner, foreground process, idle time, and verdict per tab; dry run changes nothing.
- Creating a headless agent by any of the three creation paths yields a recorded viewer tab; closing it or killing its pane is healed on the next pass, in canonical position (liaison first, coordinator viewer second).
- A live agent recorded `launch_failed` with its process proven as its pane's foreground reads `running` after a pass; a `launch_failed` record with no such proof is untouched.
- Label drift on a claimed tab is renamed back by the pass.
- The sentinel `agents/agent.json` never appears as an agent in any listing.

# Notes

Related: GitHub #345 tab-management (the public request this answers), #1225 workspace-goal-reconciler (viewer goal; Phase 1 landed), #979 (hygiene pass origin), #849 coordinator-tab-first, #670 agentctl-console. Hygiene and tab order are Rust-edition only (the Python edition refuses `watch`/`restore`); `agentctl tabs` is likewise Rust-only and the Python edition keeps refusing it rather than growing a second implementation. Canonical order: `rs/agentctl/src/tab_order.rs`; pass: `hygiene_pass` in `rs/agentctl/src/restore.rs`; planner: `rs/agentctl/src/tabs.rs`.
