---
title: 'watcher-api-error-idle: watch-agents misses an agent left idle after an API error ended its turn'
status: open
priority: 2
issue_type: task
labels:
- agentctl
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:43.603890283+00:00
updated_at: 2026-10-28T13:58:43.603890283+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-27). An agent sat idle about two hours after an 'API Error: EPROTO TLS handshake failed' ended its turn with no DONE: or BLOCKED: verdict, and watch-agents sent no idle-without-verdict notice; a check-only pass reported notices but no episode. Wanted: an API-error turn end counts as a turn end without a verdict, so the idle notice fires (and the auto-recover path can retry it when enabled). Check whether the transcript-aware notices of #574 treat the error line as activity.
