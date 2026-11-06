---
title: 'restore-in-place: restore reuses Herdr auto-restored tabs, labels parked lanes, reports per-lane disposition'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-06T21:54:45.341077072+00:00
updated_at: 2026-11-06T23:38:32.020673901+00:00
---

# Description

Item 5 absorbing #949 restore-muse-lanes. After restore, Herdr shows exactly the pre-reboot layout: lanes resume with their recorded session in their recorded (or uniquely-matching orphan) pane, duplicate bare shells closed, parked lanes labelled 'parked: NAME', per-lane report resumed-in-place/parked/failed, idempotent. Design in ai_docs/transient/2026-11-06-restore-in-place/. Slice 1: census, dispositions, parked labelling, duplicate closing. Slice 2: orphan-pane revive reuse + session fallbacks.
