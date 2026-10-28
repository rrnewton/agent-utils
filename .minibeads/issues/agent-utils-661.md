---
title: 'herdr-local-scrollback: port local scrollback onto herdr 0.9 (sidecar first, then an optional capability)'
status: open
priority: 4
issue_type: task
labels:
- agentctl
depends_on:
  agent-utils-659: blocks
  agent-utils-629: blocks
  agent-utils-652: blocks
  agent-utils-657: blocks
  agent-utils-653: blocks
  agent-utils-630: blocks
  agent-utils-624: blocks
created_at: 2026-10-28T17:39:22.102549696+00:00
updated_at: 2026-10-28T17:39:22.248867020+00:00
---

# Description

[opus 5.5] owner 11:40 ET. After everything else requested in the last 24 h (deps: #624, #629, #630, #652, #653, #657, #659): port the July local-scrollback prototype (branch feat/responsive-local-mirror, 757 commits behind) onto herdr 0.9 as cleanly as possible: an optional endpoint capability (negotiated like surface_scroll, #3509) for history-by-range then append-only output deltas; client scrolls locally. Note herdr accepts PRs only from invited contributors: open it on the owner's fork and link it from discussion #1397 (draft update in /tmp/dev-agent-utils-handoffs/herdr-web-research.md); consider the sidecar-on-observe path first. Research report: /tmp/dev-agent-utils-handoffs/herdr-web-research.md (copy at ~/agent_scratch/herdr-web-research-20261028.md). The implementation doc must include a short section on what changed in herdr between the prototype's base and 0.9 (#3487 client-side UI, #3509 versioned capabilities, #4711 surface_scroll, observe/control streams) and how the new approach differs from the old prototype (owner 11:40 ET).
