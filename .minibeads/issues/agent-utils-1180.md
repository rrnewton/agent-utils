---
title: 'gate-owner-priority: fast-track owner-requested gates ahead of the queue'
status: closed
priority: 0
issue_type: feature
assignee: mu-gateprio
created_at: 2026-11-10T04:10:00+00:00
updated_at: 2026-11-10T07:10:36.233476557+00:00
closed_at: 2026-11-10T07:10:15.130652815+00:00
---

# Description

Owner request (2026-11-09, replies 286 and 288): user-interface changes the owner asks for must be
fast-tracked through the validation gate, and gates must not serialize one ticket at a time when they
could run in parallel. Today `scripts/gate.py` admits queued tickets in arrival order (with a rerun
tier), so an owner-requested page change waits behind every earlier ticket — today ~14 tickets at
~20 min of exclusive phase each, served one ticket at a time across all lanes (~5 h).

This issue covers Part A: an owner-priority tier in `scripts/gate.py`. A ticket marked owner-priority
(for example `GATE_PRIORITY=owner`, recorded in the ticket file with a reason string such as an issue
number, and printed by `gate.py status`) is admitted ahead of every queued ticket, rerun tier
included. It never preempts a running gate or a gate already in its exclusive phase.

Admission is decentralized (each waiting gate.py decides for itself from the shared admission
queue/runs directories). Old processes will not know the new field, so the new tier must work safely
alongside them: the slot count still comes from the runs directory, and two processes must never both
admit past the cap. An existing anomaly to explain while in the code: at 9:05 PM ET on 2026-11-09 a
gate queued at 8:51 PM ET (watcharch) was running while shared tickets queued at 6:17–6:42 PM ET
(inbox, roles, drill, docreader; all GATE_SLOTS=3) still waited.

Part B (measured exclusive-phase bottleneck report with batching/splitting proposals) is delivered as
`ai_docs/2026-11-10-gate-exclusive-bottleneck.md` under this same issue.

# Design

- Ticket file gains an optional priority field (`GATE_PRIORITY=owner` plus a reason string); writers
  record it, `gate.py status` prints it, and absent/unknown values mean normal priority.
- Admission order becomes owner tier first, then the existing rerun tier, then arrival order. Within
  the owner tier, arrival order holds.
- No preemption: running gates and gates already in their exclusive phase are untouched.
- Mixed-version safety: the cap is enforced from the shared runs directory under the existing
  admission lock, so an old process that ignores the new field cannot cause an over-admission; an old
  process may order owner tickets by arrival (degraded fairness) but never past the cap.
- Document the tier in the gate help text and the user guide.

# Acceptance Criteria

- An owner-priority ticket queued behind normal and rerun tickets is admitted first; ties among owner
  tickets keep arrival order; a running gate is never preempted.
- Mixed-version test: an admission decision made without the new field (old-code path) racing a
  new-code admission never exceeds the slot cap.
- `gate.py status` prints the priority and reason for owner tickets; help text and user guide
  describe the tier.
- The watcharch anomaly (9:05 PM ET, 2026-11-09) is explained from admission history and code, with a
  fairness verdict; any fairness bug found is fixed or filed.
- The exclusive-phase bottleneck report (Part B) lands with measured numbers and per-proposal
  estimated savings.

# Notes

Filed by mu-gateprio for coordb from owner replies 286/288 (2026-11-09 9:00–9:01 PM ET). Slot
gateprio, branch mu-gateprio/gate-owner-priority. Tests use a temporary queue directory; the live
admission queue is read-only.

[watermelon-full-preview] Closed by mu-gateprio 2026-11-10. Part A landed f75f1185 (owner tier in scripts/gate.py: ahead of reruns, FIFO among owners, no preemption, mixed-version cap via runs/ under flock; shared-FIFO poll-race fairness bug fixed). Live proof: own gate admitted 115s from position 1/7, exclusive phase inherited owner tier, cap never over 3; seven owner-UI lanes used the tier the same night. Watcharch anomaly: 6:14-8:54 PM exclusive convoy = documented rerun policy with no aging floor (policy gap); 8:54:59 PM jump = fairness bug, fixed. Part B: harness ai_docs/2026-11-10-gate-exclusive-bottleneck.md (8402bc4). P0 split to #1190. Gate exceptions named to coordb: lint-only rerun for 2 unused type ignores; wrkslots shard-3 waived under #1188 signature.
