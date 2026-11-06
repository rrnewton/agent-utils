---
title: 'merge-queue-close-merged: treat already-merged and already-closed pull requests as done in the close drain'
status: closed
priority: 2
issue_type: task
created_at: 2026-11-06T21:23:48.971810+00:00
updated_at: 2026-11-06T21:49:33.857223+00:00
closed_at: 2026-11-06T21:49:33.857223+00:00
---

# Description

Follow-up to #973 merge-queue-close-replayed-pr (landed a761c7c3).
The serve close drain retries `close_pending` items about once a
minute. A consuming queue reports (paraphrased): of its 11
close_pending items, 6 are already CLOSED on the hosting service
(hand-closed) and 5 are already MERGED (the exact-SHA fast-forward
made the hosting service mark them merged). `gh pr close` exits 0
on an already-closed pull request but exits 1 ("can't be closed
because it was already merged") on a merged one, so the 5 merged
items would log close-failed every minute forever and spend about
5 hosting reads per minute of the queue's API budget.

Wanted:

1. Before closing, the closer decides from a state READ, never
   from a close attempt, and batches the reads: one
   `gh pr list --state all --json number,state` (or one GraphQL
   query) for all close-pending pull requests per cycle, in the
   same budgeted hosting client (no unbudgeted calls, no per-PR
   `pr view`: on the reporting host every hosting call goes
   through a paced wrapper counted as a write, 1 per 30 s). It
   treats MERGED or CLOSED as done: record the item closed in
   the ledger and log one line ("already merged on GitHub;
   nothing to close" / "already closed"). Also classify the
   hosting tool's "already merged" close error as done if it is
   seen anyway (race between the state read and the close).
2. A close that keeps failing for another reason backs off
   (exponential, to an hourly cap) instead of retrying every
   minute forever; the log line keeps naming the pull request and
   the hosting error.
3. Cap the closer's hosting calls per cycle (at most N closes
   per cycle, configurable, default 1) so it cannot starve the
   queue's other hosting work; state-done items cost no close.
4. Tests with a fake hosting CLI covering: open -> closed;
   already CLOSED; already MERGED (state read); "already merged"
   error on close; persistent other error backs off; per-cycle
   call count; budget accounting unchanged or reduced.
