---
title: 'gate-admission-fairness: bound every gate ticket wait with aging, keeping the owner fast track'
status: closed
priority: 1
issue_type: feature
assignee: mu-gatefair
closed_at: 2026-11-11T08:30:00.000000000+00:00
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

Goal: keep the owner's explicit fast-track rule (owner UI gates go to
the head of the queue, behind only a gate already in its exclusive
phase) while bounding how long normal tickets can starve behind a
stream of owner admissions, and make starvation visible early.

# Design

Final design (owner decision, 2026-11-10 3:19 PM ET): INTERLEAVE,
replacing the aging, time-credit and visibility-only designs recorded
in earlier revisions of this issue. Those are withdrawn.

- Admission order stays tier-first: owner, then rerun-marked, then
  ordinary, FIFO within each tier.
- One rule added: after N consecutive owner-tier admissions, if any
  non-owner ticket is waiting, the oldest non-owner ticket is admitted
  next. N is a host setting, default 2 (environment and per-host-file
  knob, in the pattern of the other gate host settings).
- The rule is computed in the shared ordering function from the
  admission history plus the current runs, ordered by started_at:
  count the trailing consecutive genuine-owner admissions, and when
  the count reaches N the oldest waiting non-owner ticket sorts ahead
  for the next grant. Nothing is rewritten and nothing is promoted:
  ticket files are never modified to change their tier, so mixed
  versions agree — an old waiter that ignores the rule just sees
  FIFO-by-tier, the flock around check-and-write is unchanged, and the
  slot cap cannot be exceeded. A nested exclusive phase keeps its
  existing rerun-tier behaviour.
- Tickets an external stopgap marked with an interleave: reason are
  normal tickets for every ordering and counting purpose, never owner
  tier.
- Queue view: tickets record their cwd (a field older code ignores);
  gate.py queue prints position, class, priority, age and cwd for
  running and queued tickets, and a warning line names the oldest
  normal (non-owner) wait once any normal ticket has waited more than
  2 h, as does a waiting gate's own output.

# Acceptance Criteria

- Five owner tickets and three normal tickets queued behind a busy box
  admit in the order O O N O O N O N; after a non-owner admission the
  consecutive-owner count restarts.
- A nested exclusive phase keeps its rerun-tier behaviour; FIFO holds
  inside each tier when the interleave rule is not due.
- A ticket marked with an interleave: reason orders and counts as a
  normal ticket, and no ticket file is rewritten by the rule.
- gate.py queue shows position, class, priority, age and the ticket
  cwd for queued and running tickets, including tickets written before
  the cwd field existed, stays read-only, and warns when a normal
  ticket has waited more than 2 h.
- Focused tests in py/tests/test_gate_admission.py pass; the full gate
  runs at normal priority. This is P0 infrastructure (owner decision).

# Notes

Filed by mu-gatefair from the 2026-11-10 admission evidence above.
Design history: a 30 min aging threshold, then a 120 min owner credit,
then a visibility-only design, all withdrawn 2026-11-10; the shipped
design is the interleave rule above (owner decision, reply 344). A
stopgap on the gate host promotes the oldest normal ticket after two
owner admissions until this lands; its interleave:-marked tickets are
normal tickets here.

[watermelon-full-preview] Closed by mu-gatefair 2026-11-11. Landed
8e1beccd on github/main: interleave in scripts/gate.py (after 2
consecutive owner admissions, host setting default 2, the oldest
waiting non-owner goes next, from runs plus history by started_at,
nothing rewritten or promoted; interleave:-marked tickets count as
normal), cwd in tickets, queue view with position/class/priority/age/
cwd and the over-2h normal-wait warning. Focused suite 54 passed;
combined tree with main verified 62 passed; full gate only red was
vibe-talk.check.approval-card-phone-help-browser, untouched, solo
green locally (owner reply 346). Earlier aging and credit builds were
withdrawn unlanded and their promoted live tickets were demoted by
the coordinator.
