---
title: 'agent-utils-merge-queue-dogfood: turn on our own merge queue for agent-utils main'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-12T12:00:00.000000000+00:00
updated_at: 2026-11-12T12:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner reply 004 (2026-11-12): "Seems like we should maybe turn
on our own merge queue if we haven't already with all these PRS. It could do batching.
Does 030 team have a good way to submit its prs to the queue?" Today every agent-utils
lane on devbig014 runs its own full gate (scripts/gate.py over scripts/validate.py,
shared admission at /tmp/dev-agent-utils-gate-admission, GATE_SLOTS=3, owner-priority
tier) and lands with bin/land-main (descends-from-main check, overlap refusal, replay).
Our own rs/merge-queue (#732/#733, closed) already runs another team's queue on this
host; its bench measured 1.6-3.7x over serial landing. Phase 1 is the plan (no code
until approved): /home/newton/work/dev-agent-utils/ai_docs/plans/2026-11-12-agent-utils-merge-queue-dogfood.md
(dev-agent-utils harness repo). The plan's core finding: the queue is ready; the
consumer adapter is not. validate.py and gate.py emit none of the queue's refusal
signals (could-not-run scope, admission line, early-red cause), assume a fresh
origin/main base instead of the job's base, and would run a second scheduler beside
the queue's donor leases.

# Design

- One queue on devbig014 for agent-utils main. Lanes submit instead of self-gating:
  `merge-queue submit <branch> --tested-with ...` on 014; a pushed ref
  `refs/mq/<lane>/<slug>` plus an agent-teams enqueue message (full SHA, ref, issue,
  slug) from devbig030. The queue verifies ref==SHA, shared-bot authorship,
  harness-proved sender, and descent from the job's base. No ssh, no shared
  filesystem, no new token, no secrets in messages.
- Config: `--validator dagrun --dag validation.dag.yaml --select 'python3
  scripts/validate.py --base "$MQ_BASE" --list' --id-prefix 'STEP ' --policy
  path-routing --land push`. Batch selection is the union of touched areas (plus
  always-on groups and reverse dependencies) computed against the batch's parent.
  The queue's push driver replaces land-main's replay/overlap logic in this path;
  land-main remains the fallback and the rollback.
- Priority: owner-UI fast-track maps to the existing `--prioritized` (planned first,
  own batch, own green) plus `--priority 0`. Deploy-on-landing subscribes to the
  queue's on_land hook, polling `list --json` until the hooks-log fix lands.
- One scheduler: the queue owns admission; gate.py becomes a lease client. A
  validation starts only on a queue lease plus gate.py's host check (host-quiet rc 0
  and no active measurement hold), folding #1406's deferral into donor admission.
  dagrun keeps per-gate CPU containment.
- Builders: mu-gatedefer (devbig030) owns validator signals, the queue-mode base
  check, and gate.py's lease mode; a fresh devbig030 lane owns the submit client;
  mu-mqmismatch (devbig014, crate owner) owns queue config, landing #1343, and
  verifying/closing #930/#931/#935 (code already on main); coordb owns cutover.

# Acceptance Criteria

- Plan approved by coordb (Phase 1 gate: no code before approval).
- Validator prints exit-75 could-not-run scopes including `job stale-base`, the
  admission line, and early-red cause; queue-mode checks "target contains job base".
- Shadow/compare soak (after #1343 lands): zero unexplained mismatches for 2 days,
  shadow throughput at or above the per-lane baseline; rollback drilled once.
- Cutover after the day's Discuss landings: lanes submit, queue lands via push,
  owner-UI tier first; `--close-prs` off until #1360 lands.
- Rollback is one switch: stop the daemon, lanes resume gate.py + land-main.

# Notes

Blocker triage (read 2026-11-12): #930/#931/#935 have code on github/main
(89572336, 94cefa6d, b7bf6055) with minibeads still open — verify and close, do not
rebuild. #1343 is fixed on mu-mqmismatch/compare (3e592901), not yet on main; it
blocks shadow sign-off only. #1357 (hooks log), #1358 (stale router drainer) and
#1360 (close budget) are in progress with workarounds in the plan. #1062/#1066 are
P2 donor-fidelity items, not day-one. Estimates: 42-58 h build across three lanes,
4-6 days elapsed including shadow. Filed by mu-mqdogfood; coordinator coordb.
