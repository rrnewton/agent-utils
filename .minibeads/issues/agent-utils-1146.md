---
title: 'gate-exclusive-phase-starvation: later --rerun tickets keep jumping ahead of a full gate''s exclusive phase'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T17:20:00.000000000+00:00
updated_at: 2026-11-09T22:30:00.000000000+00:00
closed_at: 2026-11-09T22:30:00.000000000+00:00
---

# Description

`scripts/gate.py` grants queued tickets "reruns first, then FIFO"
(`Admission.tickets`: `out.sort(key=lambda item: 0 if item[1].get("rerun") else 1)`).
A full gate's exclusive phase (validate.py's gate-exclusive phase) releases the
parent's slot and queues a FRESH exclusive ticket, with a new `queued_at` and
no rerun mark. So every `--rerun` ticket queued later still sorts ahead of it,
with no bound.

Observed on the shared gate host, 2026-11-09:
- a full gate passed its shared phase at about 11:40 AM ET;
- its 5-step exclusive-phase ticket then waited more than 3.5 h;
- later exclusive rerun tickets, one queued about an hour after it, kept being
  admitted ahead;
- meanwhile 4 shared full gates waited behind every exclusive run.

The full gate cannot finish, its tree cannot land, and the queue behind it
backs up.

# Design

Measure first: from `/tmp/dev-agent-utils-gate-admission/history.jsonl`
(both hosts, if available), give the wait-time distribution of exclusive-phase
tickets versus reruns, and how often a rerun queued later was granted first.

Then bound the rerun priority. For example:
- reruns jump ahead only while the oldest waiting exclusive-phase ticket is
  younger than N minutes; or
- a nested exclusive-phase ticket inherits its parent run's original
  `queued_at`, or a rerun-equivalent priority, so it is FIFO with the reruns.
Keep FIFO among reruns, and keep the existing deadlock-free nested release.

Add `gate.py queue --explain`: for each queued ticket, say why it waits (which
running run it waits for, which tickets are ahead and why), so this kind of
starvation is visible without reading the code.

# Acceptance Criteria

- An exclusive full-gate phase waits behind at most a bounded number of later
  reruns, or a bounded time, whichever the design picks; a test covers it.
- Reruns still go ahead of ordinary shared tickets.
- `gate.py queue --explain` lists every ticket with its reason for waiting.
- The measurement (before and after) is recorded in Notes.

# Notes

Filed for the lead coordinator; it goes to whichever lane frees up first.

Measurement before (the lead, 2026-11-09, from each host's
`history.jsonl`, phases identified by their cross.* `--max-steps 1`
command): on the shared gate host 30% of exclusive-phase tickets were
overtaken by a rerun queued after them (worst wait 200 min); on the other
host 14% (worst 258 min). The after measurement waits for the coordinators
to turn the switch on; history and ledger rows now carry `nested_phase`,
`rerun` and `phase_priority`, so it no longer needs command inference.

Approved design (the lead, 2026-11-09): a genuine nested exclusive phase
takes the existing rerun ordering mark when a per-host switch is on; aging
and an inherited `queued_at` were rejected because mixed old and new
clients would deadlock or ignore them. Accepted tradeoff: a marked phase
may pass shared tickets queued before it (at most one phase run).

## Resolution

[opus 5.5] Delivered in `scripts/gate.py`, default off:

- A nested exclusive gate whose `GATE_ADMISSION_RUN_ID` names a runs/
  ticket with a live holder (the `_alive` check `_reap` applies before a
  grant) enqueues with `rerun: true` plus `nested_phase: true` and
  `phase_priority: <switch source>`, only while the switch is on. A stale
  parent file or the variable alone gives no mark. `tickets()` is
  unchanged, so later reruns cannot pass a marked phase but still pass
  shared tickets; parent-slot release and one exclusive at a time are
  untouched.
- Switch: `~/.local/etc/agent-utils/gate-phase-priority.toml` holding
  `phase_priority = true` (path override `$GATE_PHASE_PRIORITY_CONFIG`) is
  the operator switch; `$GATE_PHASE_PRIORITY=1/0` is the test and override
  hook and wins. Read once at enqueue. A malformed file means off plus a
  printed warning and never blocks a gate. Not turned on anywhere.
- History and ledger rows gain `rerun` (the caller's own `--rerun`),
  `nested_phase` and `phase_priority`; the ledger kind is unchanged.
- `gate.py queue [--explain]`: runs and tickets in grant order with age,
  mark and command; per ticket its blockers through `grant_blockers`, the
  same function `try_grant` now uses (running runs, tickets ahead with
  rerun tier or FIFO, a full shared box, a blackout); a starvation line for
  a phase ticket (marked, unmarked or inferred from a legacy command); and
  the switch state and source.
- Audit of `rerun` readers: the RERUN_MAX_STEPS check and the preflight
  skip key off the caller's `--rerun` flag, so a five-step phase is neither
  refused nor truncated and keeps the "nested gate" preflight record;
  blackout handling follows the parent admission; the ledger kind derives
  from the class. No other code in the repository reads a ticket's
  `rerun`. Mixed versions: an older gate.py reads another lane's ticket
  only to sort it (`tickets()` by `rerun`, then filename) and to apply the
  class rules in `try_grant`; its max-steps and blackout checks look only at
  its own command. Checked by running the base gate.py's `status`,
  `tickets()` and `try_grant` against marked tickets: it orders and grants
  the marked phase ahead of a later rerun.
- Tests in `py/tests/test_gate_admission.py` cover each condition.

Rollout is the coordinators': watch `gate.py queue --explain`, then turn
the switch on one host first.
