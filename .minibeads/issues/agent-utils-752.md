---
title: 'agentctl-stop-muse-hook: stopping a Muse agent must reap its review hook so the slot can be removed'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.722799653+00:00
updated_at: 2026-11-02T16:26:35.722799653+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). agentctl stop of a Muse agent leaves muse-session-review-hook --wait running with cwd in the slot, so wrkslots remove refuses (mu-voiceleak, mu-recap 2026-10-31). stop should reap the harness's process group/children.
