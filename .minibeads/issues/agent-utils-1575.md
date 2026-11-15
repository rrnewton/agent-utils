---
title: 'per-slot-journal-scoping: one dead journal must not block unrelated slots'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Two dead-owner journals on one validate
slot refused every mutation on the registry, including adopting a
brand-new slot. `_assert_no_journal` gates on any outstanding
journal across shards; dead-actor auto-recovery covers only
absent-row kinds. Scoped create/finish journals already carry slot
identity, so the global blast radius is a scoping defect, not a
safety requirement. Model flag PERSLOT in #1568.

# Design

Scope the journal gate to journals overlapping the mutation's slot
and machine shard (batch-seal journals keep their declared target
set). Widen dead-actor auto-recovery only where revalidation proves
the recorded intent is still exactly resumable. Legacy unscoped
journals keep the global gate with their current wording.

# Acceptance Criteria

- A dead lifecycle journal for slot A does not block create/adopt/
  remove for disjoint slot B on the same shard.
- Overlapping and legacy journals still block with named remedies.
- Recovery after coordinator restart has an authorized caller
  (with #1570 delegation).
