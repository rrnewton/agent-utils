---
title: 'stale-task-triage: close, re-scope or block the open issues nobody has touched'
status: open
priority: 2
issue_type: task
labels:
- tasks
depends_on:
  agent-utils-633: blocks
  agent-utils-631: parent-child
created_at: 2026-10-28T13:57:49.472588219+00:00
updated_at: 2026-10-28T14:20:48.524942564+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters (near-term fix 5). On 2026-10-28 13 open issues had not been updated in 7 days and 7 were at least 14 days old (#28 delegated-cpuset, #29 delegated-scope-smokes, #208 persistent-cache, #238 agentctl-rust-only, #275 voice-auto-reconnect, #293 host-admission, #294 dagrun-wall-timeout-multiplier; local issues, not the GitHub issues with the same numbers). An open issue nobody will do is noise in mb ready.

Wanted: for each, one of: close (done, superseded, or not planned, with the reason), re-scope with a current description, lower the priority, or add the real blocking dependency so mb ready stops listing it. Run after #633 backlog-to-tracker so the migrated items are triaged in the same pass.
