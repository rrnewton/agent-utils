---
title: 'gate-exclusive-phase-starvation: later --rerun tickets keep jumping ahead of a full gate''s exclusive phase'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T17:20:00.000000000+00:00
updated_at: 2026-11-09T17:20:00.000000000+00:00
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
