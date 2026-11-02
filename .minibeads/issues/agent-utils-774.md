---
title: 'registry-layout-marker-refused: agentctl refuses the layout-2 guard marker in .renames instead of skipping it'
status: open
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T16:44:58.645310944+00:00
updated_at: 2026-11-02T16:44:58.645310944+00:00
---

# Description

[opus 5.5] 2026-11-02: on one host, 'agentctl send coordinator' against a consumer team's layout-2 registry failed with 'unexpected entry in rename journal directory: .agentctl/.renames/upgrade-agentctl-registry-layout-2'. That file is the layout-2 GUARD MARKER written by the migration (its text: 'This registry uses agentctl registry layout 2. agentctl builds older than layout 2 stop at this file; upgrade agentctl. Readers of rename journals skip it.'), not a stale journal entry; a matching marker sits at agents/agent.json. The rename-journal reader must skip the layout marker as its own contract says. The same marker exists in registries where commands work, so find which code path (send/drain/lifecycle) reads the journal strictly. Regression test against a migrated registry fixture. Do not delete the marker.
