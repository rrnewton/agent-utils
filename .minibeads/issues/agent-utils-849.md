---
title: 'coordinator-tab-first: agentctl moves a coordinator''s Herdr tab to the front'
status: closed
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T19:09:00.204876716+00:00
updated_at: 2026-11-05T13:21:53.000000000+00:00
---

# Description

The owner: coordinator tabs come first in Herdr. Today a newly launched coordinator's tab lands wherever Herdr puts it, and someone has to move it by hand.

# Design

- When agentctl launches or revives an agent with the coordinator profile (or designates a coordinator), move its tab to the front of its workspace.
- Herdr's CLI has no verb for this. Use the socket API: send `tab.move {"tab_id":"<workspace>:<tab>","insert_index":0}` as one JSON line to the Herdr socket (`$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`). The reply is the new tab list.
- Best-effort: a failed move is logged and never fails the launch. With several coordinators in one workspace, keep their relative order.

# Acceptance Criteria

- Tests with a fake Herdr socket: a coordinator launch sends tab.move with insert_index 0; a worker launch sends nothing; a socket error does not fail the launch.
- The userguide notes the behaviour; gate passes.

# Notes

Owner reply via the lead (2026-11-04). Assigned to cl-start after #848.

## Resolution

[opus 5.5] cl-start, 2026-11-05. New module `rs/agentctl/src/tab_order.rs`, and `ManagedAgents::bring_coordinator_tab_forward` in `subagents/control.rs`.

- **When:** after `start`, `revive` (one agent, or every agent with `--all`), `adopt`, `attach-self` and `coordinator set`.
- **Who counts as a coordinator:** the designated coordinator, or an agent launched with a profile named `coordinator` or ending in `-coordinator`. Its tab moves to just after the tabs of other coordinators already at the front of its workspace, so several coordinators keep their order.
- **How:** through the Herdr socket. `tab.list` gives the workspace's order, then `tab.move` sends `insert_index`. The order is the reply's list order: Herdr's `number` is a creation number, not a position (checked read-only on this host's server).
- **Safety guard:** shells inside a Herdr pane inherit `HERDR_SOCKET_PATH`, so a test that runs agentctl with a stand-in Herdr could otherwise move the real server's tabs. The socket is therefore used only when agentctl's Herdr program is the one serving it: inside a pane, the program `HERDR_BIN_PATH` names; elsewhere, a compiled program rather than a script. The tmux backend never moves tabs.
- **Best-effort:** with no socket, nothing happens. A failure is printed on standard error and never fails the command. The JSON output is unchanged.

Tests (with a fake Herdr socket):
- `a_coordinator_tab_moves_to_the_front_after_the_coordinators_already_there` (sends `tab.move` with `insert_index` 1, behind an earlier coordinator).
- `the_designated_coordinator_tab_moves_to_the_front_whatever_its_profile` (`insert_index` 0).
- `a_worker_tab_and_a_coordinator_tab_already_in_front_are_left_alone` (a worker sends nothing).
- `an_unreachable_herdr_socket_is_an_error_for_the_caller_to_report`.
- `tab_order` unit tests: target index, profile names, and the script-versus-compiled guard.

Unverified: no real tab was moved, because that would have reordered the owner's own tabs. The first coordinator `start` or `revive` on a deployed build is the live check.
