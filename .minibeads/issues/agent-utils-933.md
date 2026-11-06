---
title: 'agentctl-agentcloud-viewer-identity: stop refuses to close an agentcloud viewer pane after agentterm is upgraded'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-06T13:00:00.000000000+00:00
updated_at: 2026-11-06T13:00:00.000000000+00:00
---

# Description

`agentctl stop` on an agentcloud agent halted the cloud session but refused to close its local viewer pane. It exited 75 with "runs something other than the recorded agentterm viewer". The agentterm executable had been upgraded after the viewer started, so the recorded executable inode no longer matched the current file at the same path. The pane held only `agentterm -s SESSION`. `agentctl doctor` reported nothing.

# Design

- Identify the viewer by its argv (the session id) plus the process start time, rather than only by executable inode.
- Alternatively, accept an executable that was replaced or deleted when its recorded path matches.
- Keep refusing a pane that runs something unrelated.
- `doctor` reports a viewer whose executable changed under it, naming the recorded and current identity.

# Acceptance Criteria

- A test replaces the viewer's executable after start. `stop` then closes the pane, and `doctor` reports the replacement.
- A pane running a different program is still refused.

# Notes

Reported 2026-11-06 8:45 AM ET by the lead coordinator, from a consumer's report. Low priority; assign when a lane frees up.
