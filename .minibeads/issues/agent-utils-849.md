---
title: 'coordinator-tab-first: agentctl moves a coordinator''s Herdr tab to the front'
status: open
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T19:09:00.204876716+00:00
updated_at: 2026-11-04T19:09:00.204876716+00:00
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
