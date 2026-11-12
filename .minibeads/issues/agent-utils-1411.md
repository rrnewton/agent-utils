---
title: 'saturation-quiet-host-caveat: saturated-scope proofs run only on a quiet host and never on a validation''s CPUs'
status: closed
priority: 2
issue_type: task
labels:
- validation
created_at: 2026-11-12T11:05:29+00:00
updated_at: 2026-11-12T11:05:29+00:00
closed_at: 2026-11-12T11:05:29+00:00
---

# Description

[opus 5.5] The #1250 proof bar (20 of 20 runs inside a saturated CPU-quota scope, with four spinners pinned to a few CPUs) said nothing about what else is running on the host. On 2026-11-11, between 23:00 and 23:14 UTC, a lane's saturated proof pinned its spinners onto CPUs that another team's merge-queue validation was using, on a host that donates CPU to that queue. That helped eject a good change on CPU inflation. A proof that loads the host is synthetic load, and it must not land on someone else's measured run.

# Design

Add a quiet-host caveat wherever the proof bar is stated: the #1250 Design, the #1403 Acceptance Criteria, and the #1410 Design.

- Start a saturated proof only when no merge-queue validation (on hosts that run one, user units matching `validate-merge-queue-*`) and no other team's dagrun is active, and outside the 00:15-04:00 ET blackout.
- Never pin spinners onto CPUs that an active validation uses.
- Keep the burst short, and stop the spinners by verified PID (or with their scope) afterwards.

#1406 gate-defer-merge-queue automates the same check for gates.

# Acceptance Criteria

- The caveat is in the proof-bar text of #1250 and #1403.

# Notes

Numbered from devbig030's interim block (1400-1469).

## Resolution

[opus 5.5] Delivered in this commit: the caveat was added to #1250's proof bar and #1403's acceptance criteria. #1410 carries it in its Design.
