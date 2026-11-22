---
title: 'gate-rerun-fair-lane: short --rerun tickets must not wait hours behind full gates'
status: open
priority: 2
issue_type: bug
labels:
- gate
created_at: 2026-11-21T19:55:00-05:00
updated_at: 2026-11-21T19:55:00-05:00
---

# Description

--rerun tickets (a few steps, minutes long) queue in grant order with full gates, so on a busy host they wait for hours: on 2026-11-21 several sat 3-5 h at queue positions 5-11 while full gates ran, delaying landings that needed only a short rerun.

# Design

Extend the owner-tier interleave: after N minutes (host-configurable, default 30) the oldest queued rerun is promoted ahead of ordinary shared tickets whenever a slot frees, never preempting running gates and never ahead of the owner tier; at most one promotion per slot-free cycle, so full gates keep FIFO among themselves and cannot starve.

# Acceptance Criteria

- A rerun queued behind full gates is admitted at the next free slot after the threshold; full gates still progress; owner tier still first.
- Deterministic in-process admission tests.

# Notes

Filed by the coordinator; the owning lane refines this file as it works.

Note (landing, 2026-11-22): test_interleaved_admission_order_end_to_end is timing/order-sensitive under load (it failed once in the #2085 gate with O5 admitted before N2, then passed locally and on a solo rerun) and should become condition-based (owner rule 346).
