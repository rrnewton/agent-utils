---
title: 'agentctl-agentcloud-viewer-identity: stop refuses to close an agentcloud viewer pane after agentterm is upgraded'
status: closed
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-06T13:00:00.000000000+00:00
updated_at: 2026-11-07T19:00:00.000000000+00:00
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

## Resolution

[opus 5.5] cl-start, 2026-11-07. The recorded identity (boot, PID, start time, executable device and inode) is read from `/proc/PID/exe`, which keeps the old inode when a file is merely replaced; it changes only when the running process executes a new image. So the viewer that `stop` refused had re-executed itself after the upgrade. A recorded viewer that has done so is now recognised when everything but the image still matches: the same boot, PID and start time, still the leader of the tab's foreground group, with `-s SESSION_ID` (this record's session) in the arguments it runs under now, read under the live identity. Then `status` reports `agentterm_attached: true` with a `viewer_reexecuted` entry (recorded and current device/inode and the new path), `stop` closes the tab, and `doctor` reports the informational finding `viewer-reexecuted` with that detail; doctor stays clean, since stop still works. A process that exec'd into anything without this session's `-s` argument, a different generation of the PID, or a process that is not the foreground leader is refused as before. Both Herdr and tmux backends. User guide updated (agentcloud step 2, doctor findings).

Tests: `client::tests::a_viewer_reexecuted_after_its_executable_was_replaced_is_recognised` (a real process whose executable file is replaced and which then re-executes it: recognised with its new arguments; another start time or a non-foreground group is not); `subagents::cloud::tests::stop_closes_the_pane_of_a_viewer_that_reexecuted_and_doctor_reports_it` and `stop_refuses_a_recorded_process_that_reexecuted_into_another_program`.
