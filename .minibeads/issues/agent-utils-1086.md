---
title: 'validate-run-authority-budget: a donor entry-authority refusal at 9.7 s on a loaded host — budget too tight or real authority gap?'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-08T19:47:11.000000000+00:00
updated_at: 2026-11-08T19:47:11.000000000+00:00
---

# Description

On 2026-11-08 at 18:55:37Z the consumer's merge queue reported a donor
refusal from devbig030, job v4 (the consumer's PR 5302):

> validate-run: REFUSED: prospective validation slot/path authority could
> not be proved after 9.720s ... wrkslots

The trailing detail after the colon was truncated in the report that reached
us, and the exact incident line could not be found afterwards in the
consumer's validate logs, validate-ledger spool, or run handles on this host
(an admission that never ran leaves no run handle). The host was heavily
loaded at the time (load average ~150 on 316 cores), with several shared
gate tickets running.

## Findings (read-only investigation, 2026-11-08)

- The refusal is raised by the consumer's validate-run entry admission
  (`require_validate_entry_cleanup`): it holds the run-writer generation and
  run-population registry locks, then classifies interrupted create journals
  by shelling out to wrkslots `classify-create-journals`, and inspects
  prospective run handles. Any `OSError`/`RuntimeError`/`ValueError` in that
  span is re-raised as this one refusal, with the elapsed split
  "(<classifying>s classifying, <waited>s waiting for registry locks)".
- The classification budget is 30 s + 4 ms per wrkslots event in the event
  chain, capped at 300 s, sized by counting the wrkslots events; the
  registry-lock wait bound is 900 s. The entry classification drives
  wrkslots subprocesses (`classify-create-journals`) through a budget-bound
  runner, so host load stretches every subprocess inside the same fixed
  budget — the same failure shape as #954's remove budget under load.
- The 9.720 s total is far below both nominal bounds, so the refusal was
  not a budget expiry on its face: either a sub-classification command hit
  its remaining-budget timeout early (because counting/classifying events
  had already consumed the budget under load), or a real authority conflict
  (overlapping slot, run handle, or interrupted create journal) was found.
  Without the inner error text these cannot be distinguished yet.
- Recording: the consumer's merge-queue adapter maps exactly this refusal
  (by its remedy line) to `MQ_COULD_NOT_RUN_SCOPE: host validation entry
  authority could not be proved` with exit 75, and the merge queue
  (validate.rs `COULD_NOT_RUN_SCOPE_MARKER`) treats a host-scope refusal as
  the host's fault: no strike against the change, job requeued, donor
  paused and probed. So provided the adapter saw the standard refusal +
  remedy lines, this was recorded as host-scope could-not-run, never as a
  red against the change. Confirming the actual receipt line for this job
  is still open (see Acceptance Criteria).

# Design

- First reproduce the evidence: get the full refusal line (inner error and
  the classifying/lock-wait split) for a refusal of this shape, and the
  donor's recorded scope line for the job, before changing any bound.
- Decide budget vs gap from that evidence: if classification subprocesses
  are being starved by host load inside a fixed budget, size or pace the
  entry classification the way #954 sized the remove item budget (or make
  the budget CPU-aware), and keep a named refusal for a real overlap.
- Keep the host-scope recording contract intact: an entry-authority
  failure that says nothing about the change stays
  `MQ_COULD_NOT_RUN_SCOPE: host ...` with exit 75, never a red.

# Acceptance Criteria

- The donor's recorded scope for the 2026-11-08 18:55:37Z job is quoted
  (host-scope could-not-run, or a correction is filed if it was recorded
  any other way).
- A loaded-host reproduction (or the recovered incident line) shows which
  phase consumed the 9.7 s: registry-lock wait, create-journal
  classification (wrkslots), or run-handle inspection.
- Verdict recorded here: budget too tight under load, or a real authority
  gap, with the evidence line that decides it. No fix is started until the
  lead approves the verdict.

# Notes

Reported 2026-11-08 by the lead coordinator from the consumer's merge
queue. The consumer is not named, per repository policy. Investigation was
read-only on the consumer side. Do not start a fix until the lead approves
the verdict.
