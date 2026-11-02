---
title: 'wrkslots-reregistered-pane: classify a slot whose herdr pane was re-registered by another agent'
status: open
priority: 2
assignee: mu-rust@devbig030
issue_type: task
labels:
- wrkslots
created_at: 2026-11-02T16:58:21.503919572+00:00
updated_at: 2026-11-02T16:58:21.503919572+00:00
---

# Description

[opus 5.5] Owner goal (2026-11-02): exactly one slot per live agent plus one per running validate, and ownerless slots past their TTL retired deterministically. From a consumer team's wrkslots cleanup audit; its B1 and B2 were fixed in 2762da0f. When a herdr pane is re-registered by a different agent (common after a restore), wrkslots classifies the slot as unverifiable. Classify it deterministically from a signal agentctl records at re-anchor time. Boundary: wrkslots owns this classification; agentctl's pane reuse and re-anchor are #744 restore-pane-reuse. The two lanes agree the signal (e.g. agentctl records the re-anchor, wrkslots reads it) through the coordinator before either codes it.
