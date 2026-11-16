---
title: 'wrkslots-slot-containment-design: per-slot cgroup proof without host-wide census'
status: closed
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-16T21:48:24+00:00
updated_at: 2026-11-16T21:48:24+00:00
---

# Description

[watermelon-full-preview] Owner request for a written design
proposal, no implementation: reconsider slot release proofs that
scan the whole host. Removal proofs on a loaded host time out
because the pre-delete census walks every host process and the
slot directory; foreign mount namespaces pin slot binds; finished
validation checkouts linger for hours. The direction to evaluate:
with proper sandboxing a workslot is completely private to the
process tree spawned inside it, so when its cgroup or namespace is
gone the slot is released by definition, with no global scan.

# Design

Deliver one design document (see Resolution) that builds on
#1300 team-pid-namespaces instead of restarting: keep its Phase 1
probes and landed Phase 2a, and evaluate superseding its parked
per-team keeper with per-slot cgroups whose `populated` flag
carries liveness and release. Cover, in order: the containment
model and what it proves; entry for agents, validations, donors
and helpers; mount privacy beside the private-bind fix;
classification of legitimate outside access; migration with the
census as fallback; failure modes; measured read-only costs;
composition with a generic validation hub lifecycle (receipt
first, bounded cleanup-pending, one sweeper identity); and a
phased plan with condition-based acceptance tests plus the
decisions left to the owner.

# Acceptance Criteria

- One committed design document under `ai_docs/plans/`, in plain
  words with a Terms section near the top, under about 600 lines,
  covering the nine topics above in order.
- The document states plainly what it changes relative to
  #1300 team-pid-namespaces and why, and accounts for the boxed-lane
  and Eden work, current isolation modes, dagrun and gate
  containment, and the open census, bind, holder and lock fixes.
- Host costs are measured read-only and reported with method; no
  heavy census run is used as evidence.
- No code changes; the change lands through the hygiene-selected
  gate.

# Notes

## Resolution

[watermelon-full-preview] Delivered as
`ai_docs/plans/2026-11-16-slot-containment.md` in the landing
commit: per-slot cgroup proof with optional namespaces, verified
entry, private mounts, classified outside access, per-slot
fallback migration, measured costs, hub composition, six landable
phases and eight owner decisions. No implementation included;
each phase needs its own issue before code.
