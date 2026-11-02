---
title: 'agent-teams-relay-large: size-aware relay posts and a JSON-only mode per provider'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.764058478+00:00
updated_at: 2026-11-02T16:26:35.764058478+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). per-provider size limits (Slack ~11k split/40k cap; Discord bots 2000 chars, embeds 4096/6000, attachments ~10MB): small = one post; large = header + body excerpt + envelope as attached file <id>.agent-teams.json (signed, authoritative); no-attachment providers = numbered signed part-posts reassembled, never partial. Owner OK with JSON-only posts (no human-readable copy) as a mode; a viewer utility (e.g. agent-teams relay show/tail) renders them. Next relay slice after #722 fix + backpressure.
