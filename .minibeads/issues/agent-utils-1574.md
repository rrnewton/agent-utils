---
title: 'branch-reregister: record an intentional branch change for live lanes'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Removal requires the checkout on its
registered branch; a review lane that checks out a review ref or
detaches HEAD permanently exits the removable state. Dead lanes
already have `remove --archive-drifted-checkouts` (archives the
found HEAD locally, never pushed), but a live lane has no supported
transition to record an intentional branch change.

# Design

A recorded branch-change event (owner or coordinator, exact
generation, old and new branch, HEAD proof) that re-registers the
checkout branch without weakening any salvage check. Detached HEAD
must first land on a named branch. Model flag REREGISTER in #1568.

# Acceptance Criteria

- A drifted live lane can re-register and later finish/remove
  through the ordinary path.
- Re-register refuses on generation mismatch, unfinished Git
  operation, or unrecorded HEAD movement during the command.
- Dead-lane behaviour (archive-drifted flag) is unchanged.
