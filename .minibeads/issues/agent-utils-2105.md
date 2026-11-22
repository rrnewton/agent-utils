---
title: 'coordinator-work-state-machine: track every dispatched work item through a state machine with stall timers'
status: open
priority: 1
issue_type: task
labels:
- coordinator
- tick-hub
created_at: 2026-11-22T09:55:00-05:00
updated_at: 2026-11-22T09:55:00-05:00
---

# Description

The owner asked for a review of coordinator follow-through. Work goes from planned to dispatched, implemented, reviewed (possibly back to fixes), approved, gated, landed, deployed and verified, but today the coordinator tracks this informally through skills and reminders, and items stall unnoticed (for example an owner fast-track change sat for hours behind a queued rerun of a known-unreliable check while its lane reported it was waiting).

# Design

A research report first: audit the coordinator skills against actual practice and what tick-hub checks today; define a work-item state machine with close semantics, owner-ETA tracking and per-state stall timers; compare (a) tightened skills plus tick-hub, (b) the minibeads tracker as the state store advanced by a deterministic CLI, and (c) a coordinator tool layer (create, dispatch, advance, escalate) inside existing agent harnesses with tick-hub escalating from that state; contrast with a harness that binds tasks to its tracker and scripts workflows, without building a custom harness. Recommend one and a first step.

# Acceptance Criteria

- The report is published with the options, trade-offs, recommendation and a first step; follow-up implementation issues are filed from it.

# Notes

Filed by the coordinator from an owner request; the research lane refines this file.
