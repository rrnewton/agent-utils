---
title: 'gate-admission-fairness: bound every gate ticket wait with aging, keeping the owner fast track'
status: open
priority: 1
issue_type: feature
assignee: mu-gatefair
created_at: 2026-11-10T15:15:00.000000000+00:00
updated_at: 2026-11-10T15:15:00.000000000+00:00
---

# Description

Evidence at 2026-11-10 about 10:10 AM ET on the gate host: 16 queued tickets
and 1 running in the shared admission directory. The oldest ticket, an
exclusive cross-step phase of a full gate, had waited about 12.8 h at the
head. Since about 17:00 PT the previous day the finished-run history is
almost all owner priority (#1180 gate-owner-priority): owner UI fast-track
tickets, but also an owner merge-queue defect, #1206 and a turnover tool.
Each owner ticket sorts ahead of every normal ticket, so normal shared
tickets, and the normal exclusive head they wait behind, almost never ran;
one normal gate waited 6.5 h overnight. The rerun tier has the same shape:
later reruns also sort ahead of ordinary tickets with no floor. The host
cap is 3 shared slots. #1196 front-starve is a different (merge-queue)
queue and is not this issue. #1044 gate-priority-ticket asked for a bound
in its first design but #1180 landed the owner tier explicitly unbounded;
this issue adds the bound.

Goal: bounded waiting for every ticket (shared, exclusive, rerun-tier
phases included) while keeping the owner fast track for fresh work.

# Design

Picked: aging. A ticket that has waited at least 30 minutes (by its
recorded queued_at) joins the owner tier, ordered by queued_at, so no
ticket queued after it can be granted ahead of it. Young owner tickets
still jump every young normal ticket, so the fast track is unchanged
whenever the queue drains within 30 minutes, which is the normal case.

- Bound: let A = 30 min. After queued_at + A a ticket can be overtaken
  only by tickets already queued ahead of it at that moment, a finite
  set of at most Q grants (plus gates already running). Its wait is at
  most A plus the time those Q grants and the running gates take; the
  grant-count bound is exact, and no later arrival can extend it. One
  mechanism covers owner starvation, rerun starvation, and the exclusive
  head: an aged exclusive ticket becomes unpassable, and shared tickets
  behind it were already waiting for it by the class rules.
- Rejected: a per-window owner quota or 1-in-K oldest-normal guarantee
  (#1044's bound). It needs a shared grant counter every client
  generation updates identically; old waiters grant without counting,
  so the bound silently breaks exactly during the mixed-version weeks,
  and it bounds only owner-versus-normal, not the rerun tier or the
  exclusive head. Rejected: letting shared tickets bypass an old
  exclusive head. That starves the exclusive ticket instead (the mirror
  bug) and still leaves owner arrivals jumping it.
- How ranking is decided today: there is no central scheduler. Each
  waiter, under the admission flock, re-reads the queue, sorts it, and
  grants itself only if nothing blocks it. The new rule therefore lives
  in that same sort and blocker check, computed from queued_at, which
  every ticket already records. No new files, no format change: aging
  reads only fields old tickets already have, and unknown priority
  values already degrade to ordinary.
- Mixed versions: old waiters ignore aging and could deadlock against
  it (an old exclusive waiter sees a later rerun ahead of it; a new
  rerun waiter sees the aged exclusive ahead of it; both wait). So a
  new-code waiter, while holding the flock on its grant path, promotes
  each aged ticket in place: the ticket file gains an aged-promotion
  mark plus the two ordering values old code already sorts by
  (owner priority with an aged reason, and the rerun ordering mark),
  keeping its original priority recorded beside the mark. Every client
  generation then computes the aged ticket at the front, the old
  waiter's own next poll grants it, and views converge within one poll
  cycle. The flock around check-and-write is unchanged in every
  generation, so the slot cap cannot be exceeded; ordering disagreement
  never affects how many run, only who, and promotion removes the
  disagreement. A genuine owner run's nested phase must not inherit
  priority from an aged-promoted parent ticket.
- Queue view: gate.py queue already lists grant order, class, label and
  age. Tickets gain a recorded cwd (old code ignores the new field;
  older tickets show none), and the queue and running lines print it,
  plus an aged-promoted label naming the wait that earned it.

# Acceptance Criteria

- A normal ticket aged past the threshold is granted ahead of owner and
  rerun tickets queued after it; owner and aged tickets together order
  by queued_at; a young owner ticket still jumps young normal tickets.
- An aged exclusive ticket at the head cannot be passed by any later
  ticket, and shared tickets behind it still wait for it.
- Mixed-version test: an aged ticket whose waiter uses the legacy sort
  and blockers, racing a new-code waiter that legacy order puts ahead,
  still gets granted (promotion is visible to the legacy sort, no
  deadlock), and concurrent mixed grants never exceed the slot cap.
- gate.py queue shows position, class, priority or aged label, age and
  the ticket cwd for queued and running tickets, including tickets
  written before the cwd field existed.
- Focused tests in py/tests/test_gate_admission.py pass; the full gate
  runs at normal priority (this is not owner UI work).

# Notes

Filed by mu-gatefair from the 2026-11-10 admission evidence above.
Threshold is a named constant with an environment override for tests.
