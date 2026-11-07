---
title: 'wrkslots-live-journal-busy: create-journal classification refuses a live non-create mutation as interrupted instead of busy'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T01:40:00.000000000+00:00
updated_at: 2026-11-07T11:50:00.000000000+00:00
closed_at: 2026-11-07T11:50:00.000000000+00:00
---

# Description

`_classify_create_journal` (py/wrkslots/cli.py, around the "cannot coexist" refusal near line 10400) and its twin in rs/wrkslotsd/src/classify_journals.rs (around line 413) refuse ANY non-create journal as "interrupted <kind> mutation ... cannot coexist with an operation that permits only verified disjoint create journals". They never check whether that journal's owner is still alive. Only the create branch checks `_process_state`.

Seen 2026-11-06 on one host. A merge-queue donor probed slots while a `finish` mutation was LIVE: the probe was refused at 01:30:14Z, the finish completed at 01:30:28Z, and doctor was clean by 01:32Z. The caller's start step reported the refusal as COULD_NOT_RUN, so the job was lost and the donor paused. A transient, healthy mutation became a hard failure.

# Design

- For a non-create journal, check its owner the way the create branch already does.
- Live owner: return a distinct, retryable result, e.g. "busy (live <kind> mutation by pid N; retry)", with its own exit status or JSON field, so callers can wait and retry instead of failing the job.
- Dead or indeterminate owner: keep today's hard "interrupted ... cannot coexist" refusal.
- Same behaviour in both editions, and callers that relay the result (the merge-queue donor's start step) treat busy as retryable.
- Performance follow-up worth noting here: re-reading a journal chain of about 69k events takes about 16.4 s (median) in the reporting environment. Consider caching or summarising the classification so a busy-retry loop does not pay that on every attempt.

# Acceptance Criteria

- A test in each edition: a non-create journal whose owner PID is alive yields the retryable busy result naming the kind and PID, not the interrupted refusal.
- A test in each edition: the same journal with a dead owner still gets the hard refusal.
- The donor/start path retries (or reports busy) on the busy result instead of COULD_NOT_RUN.

# Notes

Reported 2026-11-06 by the lead coordinator from another harness's tracker. Related: #982 (owner transfer after restart), #954 (remove item budget).

## Resolution

Landed as 5ba9c3ec (with its two parent commits) by mu-wrkslots (watermelon), on 2026-11-07.

- A non-create journal whose recorded owner is live now yields a retryable busy result: exit 75, JSON status busy naming the kind, PID and journal, and a BUSY line the merge-queue donor treats as could_not_run_busy and requeues. A dead or indeterminate owner keeps the hard "interrupted ... cannot coexist" refusal. Both editions.
- Liveness uses the full owner identity: the first parseable owner in owner order, and, when a task scope is recorded, a scope-owner match plus systemd invocation verification. A live PID whose task scope ended counts as dead, so it is refused, not busy (test_live_pid_with_ended_task_scope_refuses_not_busy; the existing abort-import test keeps exit 3).
- Validation: full selected gate (61 steps) on the final head; the one failure, rust.merge-queue.test CPU timeout under load, passed solo. An earlier version passed only a hand-picked subset and was caught by the full selection (the ended-scope regression), which is why the scope check exists.
- Not done: the performance note about re-reading a 69k-event journal chain (about 16.4 s median) remains a follow-up.
