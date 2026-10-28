---
title: 'agent-teams-coordinators-repo-url: the coordinators listing shows a local mirror path instead of the repository URL'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:23.411587831+00:00
updated_at: 2026-10-28T13:58:23.411587831+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-25). agent-teams coordinators prints, in its repository column, the origin URL of the harness checkout; when that origin is a local mirror, the column shows a filesystem path rather than the GitHub URL. Wanted: resolve through the mirror to its upstream remote (or show both), so the listing names the repository a reader can open.
