---
title: 'wrkslots-could-not-determine-route: a could_not_determine cleanup verdict comes once with an actionable route'
status: closed
priority: 2
issue_type: task
assignee: mu-rust@devbig030
labels:
- wrkslots
created_at: 2026-11-02T16:58:21.499843803+00:00
updated_at: 2026-11-03T03:07:10.623904635+00:00
closed_at: 2026-11-03T03:07:10.623904505+00:00
---

# Description

[opus 5.5] Owner goal (2026-11-02): exactly one slot per live agent plus one per running validate, and ownerless slots past their TTL retired deterministically. From a consumer team's wrkslots cleanup audit; its B1 and B2 were fixed in 2762da0f. A consumer's periodic cleanup reporter re-reports wrkslots' could_not_determine as failed every 45 minutes with no ACTION route (the reporter itself is consumer code). On the wrkslots side: make the verdict carry a stable reason code and a concrete remedy (the command or condition that resolves it), dedupe by slot and reason so callers can report once, and document the contract so consumers stop treating it as a fresh failure.
