---
title: 'agent-kind-misdetect: agentctl must identify a registered agent by its pane, not by the harness Herdr detects there'
status: open
priority: 2
issue_type: task
labels:
- agentctl
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:31.592921694+00:00
updated_at: 2026-10-28T13:58:31.592921694+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-26). A Claude Code agent ran another harness's CLI as an experiment inside its pane; Herdr then reported the pane as that other harness with status done, agentctl agent lookup failed (agent_not_found), and watch-agents called a working agent unreachable. Wanted: agentctl resolves a registered agent by its recorded pane id and pinned process, and treats a changed detected kind as a warning, not a lookup failure. Verify first whether #574 harness-adapters or the recipient checks already changed this.
