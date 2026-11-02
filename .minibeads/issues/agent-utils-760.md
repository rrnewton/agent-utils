---
title: 'wrkslots-format-edges: align schema and loader edge cases (integer width, control characters)'
status: open
priority: 3
issue_type: task
labels:
- wrkslots
created_at: 2026-11-02T16:26:35.751945796+00:00
updated_at: 2026-11-02T16:26:35.751945796+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). (d) integer width: schema.rs positive() accepts > i64::MAX while state loaders' as_int refuse it; (e) TaskScope Unicode Cf controls: Python + evidence::ScopeIdentity refuse, active_state reader accepts. Pre-existing; make all readers agree (refuse). (2026-11-01)
