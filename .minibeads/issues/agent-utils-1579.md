---
title: 'owner-binding-at-create: bind an owner generation or record explicit ownerlessness'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Six evidence rows had no verifiable
owner state. Two sub-cases differ today: `owner=None` rows can now
pass removal admission on TTL plus liveness plus handoff plus
occupancy, but a recorded generation that is merely indeterminate
blocks forever, and `recover-unbound-owner` refuses it as a
historical owner it cannot replace. Death is unprovable from
birth for rows published without a generation.

# Design

`create` refuses to publish a row with neither a bound owner
generation nor an explicit ownerless record. For existing
indeterminate generations, a coordinator evidence path supersedes
the unverifiable generation (recorded, generation-scoped) without
rewriting history. The finisher schedules the evidence collection;
model flag AUTO_UNBOUND in #1568.

# Acceptance Criteria

- New rows always carry a provable owner state or explicit
  ownerlessness.
- An indeterminate-generation dead slot reaches removal through
  the recorded evidence path; a live one still refuses.
- Historical owner records are never silently replaced.
