---
title: 'merge-queue-priority-label: a labelled change goes to the front of the queue, still landing only on its own green'
status: closed
priority: 1
issue_type: feature
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T14:04:31.711153227+00:00
updated_at: 2026-11-03T14:27:09.095683518+00:00
closed_at: 2026-11-03T14:27:09.095683387+00:00
---

# Description

[opus 5.5] Owner request (2026-11-03): "CI infra changes should be prioritized to the front of the queue ... some way of labeling so that we make this automated". A change that fixes the CI or validation infrastructure unblocks everything behind it, so it should not wait its turn behind ordinary work.

# Design

- Check first what the existing prioritised planning (3205bc24, "plan ready changes into conflict-free, prioritised batches") already does; this may only need the label wired into its priority.
- A configurable priority label (default mq-priority) that the queue honours.
- A labelled change is planned ahead of all non-priority work: validated alone, or as the FIRST change of the next batch, so a red priority change cannot hide behind, or eject, unrelated changes.
- It still lands ONLY on an exact-SHA green (fast-forward to green; validation is never skipped).
- Optional: a report suggesting the label when a change touches only CI or validation-DAG paths (a suggestion, never applied automatically).

# Acceptance Criteria

- A priority change queued behind 3 normal ones is validated first; a test covers it.
- A red priority change does not eject or delay unrelated changes beyond its own validation.
- A priority change never lands without its own exact-SHA green.
- The label name is configurable and documented in help and the user guide.
