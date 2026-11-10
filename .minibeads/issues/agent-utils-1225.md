---
title: 'workspace-goal-reconciler: agentctl watch repairs each workspace toward a declared tab goal'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-10T16:54:43+00:00
updated_at: 2026-11-10T16:54:43+00:00
---

# Description

[watermelon-full-preview] Owner request, 2026-11-10: the watcher should not only reconnect a disconnected viewer; it should keep a declaratively defined goal state for each Herdr workspace and always repair toward that goal. This is for the next daemon upgrade, except Phase 1 below, which is a live defect with its own small gate.

Today `agentctl watch` (the daemon that also does tab hygiene: `hygiene_pass` in `rs/agentctl/src/restore.rs`, closing bare shells and keeping canonical tab order) repairs only what a registered record's pane claims. A live agent whose viewer is dead, disconnected, or never created keeps no viewer, and nobody recreates it:

- an `agentterm -s` session viewing a remote (agentcloud) agent;
- an `agentctl tail --follow` or `agentctl console` tab viewing a headless agent (see #670 agentctl-console);
- the coordinator's own viewer tab, which must stay first.

# Design

A goal-state reconciler, not a reconnect hook:

- A declared layout per Herdr workspace, with no second config: it builds on `restore.json` (`<registry>/restore.json`, `RestoreConfig` in `rs/agentctl/src/restore.rs`) and the existing tab hygiene (`close_bare_shells`, canonical tab order in `rs/agentctl/src/tab_order.rs`, #849 coordinator-tab-first). It declares which tabs exist and in what order, with the coordinator's viewer first, and which viewer each live agent gets: its own TUI pane for an interactive agent, `agentterm -s` for a remote agent, and `tail --follow` or `console` for a headless agent.
- The same goal drives boot restore (`agentctl restore`), so boot restore and the watcher cannot disagree about the layout.
- `agentctl watch` repairs toward the goal on every pass: it creates missing tabs, restarts a dead or disconnected viewer in place (same tab, same position), closes bare shells, and reorders tabs into the canonical order.
- Safety rules: never kill a live agent's pane; act only on viewers and bare shells; a dry-run/report mode that says what a pass would do and does nothing; idempotent passes, so a pass over a workspace already at its goal changes nothing and reports nothing new.

## Phase 1: a headless agent's viewer tab ranks with the agent

This phase is a live defect and lands on its own small gate, right after the verdict-dedupe fix, ahead of the full reconciler.

`hygiene_pass` claims tabs only through records' `pane_id`/`tab_id` (`manager.status(name)` in `rs/agentctl/src/restore.rs`). A headless agent has no pane, so the tab running its `agentctl tail --follow` is `ROLE_UNMANAGED` (`rs/agentctl/src/tab_order.rs`) and the canonical order puts it last, even for the designated coordinator and for a chat bridge's target. #849's `bring_forward` likewise knows only pane-backed records (`rs/agentctl/src/subagents/control.rs`), so it cannot keep a headless coordinator's viewer first either.

Live effect, 2026-11-10: on another host, a repair watch moved a headless coordinator's tail tab from first to last within 17 seconds. An interim unit there moves it back every 2 seconds; that unit is dropped when this phase lands.

Fix:

- A headless record can name a viewer tab. `agentctl tail --follow` or `agentctl console`, when run inside Herdr, records the tab it runs in on the agent's record; an explicit command can set or clear it as well.
- Hygiene and #849's bring-forward rank that viewer tab with the agent's own role (`role_for` in `rs/agentctl/src/tab_order.rs`): a designated coordinator's or bridge target's viewer goes first, a lane's viewer ranks as that lane.
- No pane ids change: the viewer tab is a view of the agent, never its pane, and closing or moving it never touches the agent.

Later phases (the rest of this issue, for the next daemon upgrade):

- Detect a dead or disconnected viewer pane for a live agent and restart the viewer in place, in the same tab and position: `agentterm -s` for remote agents, `tail --follow`/`console` for headless agents.
- Create a viewer tab that does not exist at all, in its goal position.
- Express the goal in `restore.json` (or derive it wholly from records plus `restore.json`, if that proves enough) and make boot restore consume the same goal.

# Acceptance Criteria

Phase 1:

- A headless designated coordinator's tail tab stays first through hygiene passes, and no pane ids change.
- A headless bridge target's viewer tab ranks first; a headless lane's viewer tab ranks with its lane, not last.
- A record with no viewer tab behaves exactly as today.

Full reconciler:

- A pass over a workspace at its goal creates, closes, moves, and restarts nothing, and reports nothing new (idempotent).
- A live agent whose viewer pane is dead or disconnected gets its viewer restarted in the same tab and position on the next pass; a live agent with no viewer tab gets one created in its goal position.
- A pass never kills or closes a live agent's pane; it acts only on viewers and bare shells.
- A dry-run/report mode names every action a pass would take and takes none.
- Boot restore and a watch pass derive the same goal for the same registry and workspace.

# Notes

Related: #849 coordinator-tab-first (check whether it covers headless tail tabs; Phase 1 above is the finding that it does not), #670 agentctl-console, #509 agentctl-child-done-notice (the other watcher in this codebase, `watch-agents`; this issue is about `agentctl watch`). Tab order today: `rs/agentctl/src/tab_order.rs` (`role_for`, `canonical_tabs`, `bring_forward`), `hygiene_pass` in `rs/agentctl/src/restore.rs`. Viewer commands: `agentctl tail` and `agentctl console` in `rs/agentctl/src/cli.rs`; remote viewers are the `agentterm -s` tab recorded at agent launch.

## Phase 1 resolution

[watermelon-full-preview] Phase 1 landed, 2026-11-10; the rest of this issue stays open for the next daemon upgrade. A headless record can now name a viewer tab (`viewer_tab_id` on the record, shown by `list` and `status`): `agentctl set NAME --viewer-tab TAB_ID` sets it and `--no-viewer-tab` clears it, and `agentctl tail --follow` run inside Herdr records its own tab (best-effort, warned on failure). `hygiene_pass` in `rs/agentctl/src/restore.rs` claims that tab through a pure `ranked_entries` helper and ranks it with the agent's role, a viewer tab also marks its workspace managed and protects its pane from bare-shell closing, and `bring_coordinator_tab_forward_through` in `rs/agentctl/src/subagents/control.rs` brings a headless coordinator's viewer tab forward (its workspace is the viewer tab's own when the record names none). No pane id is read or changed anywhere in the phase. Tests: `a_headless_coordinators_viewer_tab_stays_first_through_hygiene_ranking` (designated view first, lane viewer as its lane, unnamed last, idempotent, no pane ids added) and `a_viewer_tab_is_named_and_cleared_without_touching_the_pane`. There is no `agentctl console` command in this tree yet (#670 is still in progress); when it lands it should record its tab through the same `set_viewer_tab` path. Unverified live: no real coordinator tail tab was moved on a live host; the first deployed hygiene pass over a headless coordinator with a named viewer tab is the live check, and the interim unit another host runs to move its coordinator's tail tab back can be dropped once that is seen.
