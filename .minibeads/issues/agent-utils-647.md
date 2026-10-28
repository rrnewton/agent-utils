---
title: 'archive-stale-branch: move the leftover design/server-side-search branch under archived/'
status: open
priority: 3
issue_type: task
labels:
- repository
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:59:03.970794226+00:00
updated_at: 2026-10-28T13:59:03.970794226+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-25). The GitHub branch design/server-side-search (a1179cef) is still at the top level; two attempts to move it under archived/ from the harness mirror were rejected with '(failed)'. Wanted: find why (a ruleset, a lease mismatch, or the mirror's refspec), move it per AGENTS.md, and note the cause.
