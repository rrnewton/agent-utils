---
title: 'watch-verdict-from-brief: take a DONE/BLOCKED verdict only from the agent, not from its echoed brief'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.719513846+00:00
updated_at: 2026-11-02T16:26:35.719513846+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). watch-agents took 'DONE:' from the BRIEF text echoed on screen (my brief template has a 'DONE:' section heading) as the agent's verdict right after start (cl-transcript, 2026-10-30). Want: only count verdicts in the agent's own output (transcript-based, or after the last user prompt). Meanwhile rename the brief heading to 'WHEN FINISHED:'.
