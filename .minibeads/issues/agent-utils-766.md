---
title: 'services-refresh-old-binary-prehealth: refresh a service whose old binary lacks chat health'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.775555850+00:00
updated_at: 2026-11-02T16:26:35.775555850+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). services refresh refuses a bridge whose current binary predates 'chat health' (pre-check runs the old binary; 014 a9d75334). Fall back to the snapshot's own health check, or --skip-pre-health. (014 lieutenant 2026-11-01)
