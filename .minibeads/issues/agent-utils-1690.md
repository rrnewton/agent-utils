---
title: 'pane-queue-linearize: one delivery queue per pane across registries'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-16T01:07:55.000000000+00:00
updated_at: 2026-11-16T01:07:55.000000000+00:00
---

# Description

Owner request (2026-11-15): two agentctl registries can each hold a record
for the same live pane, for example a coordinator's own registry and a chat
bridge's private registry that adopted the same harness. Each registry has
its own message queue. Both drains type into the one pane, and neither
knows what the other queued, typed or left in the composer. The owner asked
why there is no single queue that puts every message for a pane in one
order.

What is shared today: the host-wide target lock, keyed by pane id
(pane_lock_digest), so two drains never type at the same moment. What is
not shared:

- Order. The two queues interleave by lock race, not by admission time, so
  a later message from one registry can go in before an earlier one from
  the other.
- Typed-prompt memory. Composer recovery (recover_typed_draft, stranded
  reconciliation, the stale-composer sweep) matches the screen only against
  its own queue's typed prompts. A draft left by the other registry looks
  foreign, so it gets refused, backed up and cleared, or waited on, instead
  of being submitted or quarantined as possibly submitted.
- Pending state. Each queue's status, cancel and possibly-submitted view
  covers only its own records.

#1683 pane-lock-held-by-stuck-sender covers part of this: the bounded hold
caps how long either drain can keep the shared target lock, so one
registry's stuck submission can no longer block the other's for long, and
its recovery that needs no parseable composer narrows the "unparseable
composer" refusals. It does not give one order, and it does not let one
registry recognise the other's typed prompts.

# Design

To be settled in the lane. One direction to weigh: key the delivery queue
by resolved pane identity rather than by registry and agent name, in the
account-state root next to the host-wide target lock. Every registry's send
enqueues there; one drain per pane delivers in admission order; per-record
provenance (registry, agent name, sender) is kept for status, cancel and
replies; recovery consults that one typed-prompt set. Things to handle: a
record that moves to another pane (re-anchor), a pane id reused after a
restart, older editions that still drain per-registry queues during a
rolling upgrade (they must keep working and must not deliver twice), and
the existing no-O(N)-beside-fixed-files layout rule.

# Acceptance Criteria

- Two registries with records on the same pane: messages from both are
  typed in admission order (a test enqueues alternately and checks the
  order on the pane).
- A prompt typed by registry A and left in the composer is recognised by
  registry B's drain or sweep as a known typed prompt (submitted or
  quarantined as possibly submitted), never cleared as foreign and never
  retyped.
- status for either registry shows the pane's pending items with their
  provenance; cancel works only on one's own items.
- Mixed-edition operation is documented and tested: an old per-registry
  drain and a new per-pane drain on one pane neither deadlock nor deliver
  one message twice.
- Checks wait on conditions, not wall-clock budgets (owner rule on
  real-time-sensitive tests).

# Notes

Related: #1683 (bounded pane-lock hold, composer recovery without a
parseable composer), #884 (no lock held while waiting for a paste), #703
(no lock held while waiting for readiness), #1560 (stuck-paste backstop).
