---
title: 'rerun-shared-class: non-timing reruns run in the shared gate class'
status: closed
priority: 1
issue_type: task
assignee: mu-gatefast
created_at: 2026-11-10T22:40:00+00:00
updated_at: 2026-11-10T22:40:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 344 (2026-11-10): no gate needs the whole
host. Every --rerun used to take the exclusive class, so a two-minute
solo rerun of ordinary steps held the host alone and queued behind
every other exclusive ticket. Part 1a of the gatefast lane: a rerun
whose selected steps include no timing-sensitive step runs in the
shared class; only steps labelled `gate-exclusive` in the DAG (or
unknown steps, failing closed) take the exclusive class.

# Design

- gate.py reads the flattened validation DAG's labels: a step is
  timing-sensitive only when labelled `gate-exclusive`; an unknown step
  is treated as timing-sensitive (fail closed).
- A non-timing rerun is admitted shared, keeps its rerun tier in grant
  order, and is recorded in the ledger as kind `exclusive-rerun`.
- Timing-sensitive reruns keep today's exclusive behaviour unchanged.

# Acceptance Criteria

- Focused tests in py/tests/test_gate_admission.py cover the label
  lookup, the shared admission, the exclusive fallback for unknown and
  gate-exclusive steps, and the ledger kind.
- Full gate: touched steps pass; any untouched red gets one local solo
  pass (owner reply 346) before landing.

# Notes

Filed provisionally as #1251, which main took for scroll-single-owner;
renumbered to #1260 at landing (1258/1259 are mu-testaudit's). An
earlier renumber renamed a neighbouring lane's issue file by mistake;
that rename is reverted in the landing tree.
