---
title: 'merge-queue-leased-unadmitted-root: a leased rooted job counts as started before its donor admits it'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-07T22:05:00.000000000+00:00
updated_at: 2026-11-07T22:05:00.000000000+00:00
---

# Description

A rooted donor job is exempt from the stale-base sweeps as soon as a
donor leases it, but the lease is only a claim: the donor may then
wait on tool-materialization or admission locks for minutes before
any validation is admitted. If the base moves in that window, the
run's stale-root handling treats the job as started and keeps it, so
the validator later spends the slot refusing it as STALE-BASE
instead of the job being cancelled with no strike and the batch
recomposed on the current base.

Seen on 2026-11-07 in a consuming queue at 98f4b65d (run
20261107T165537Z): a rooted job was dispatched on the old base, the
base moved 3 s later, the job sat "leased to local" for about 2 min
on tool locks, and the donor slot was wasted on the stale refusal.

# Design

- A leased job counts as unstarted until the donor reports
  admission just before the validator starts (after tool
  materialization, identity refresh, fetch and checkout). The donor
  declares the capability at claim time; a lease without the
  declaration keeps the old lease-equals-started rule. The queue-side
  `was_started`/`root_unstarted` answers, the claim-time stale
  sweep, and run-side pruning all treat leased-but-unadmitted as
  unstarted. The field must default to not admitted so records
  written before this change parse.
- A rooted job a donor has admitted keeps today's kept behaviour:
  green is requeued and red is decided.
- Fallback if an admission report proves impractical: the donor
  re-checks that the job candidate still contains the current base
  immediately before admission and gives the job back when it does
  not.

# Acceptance Criteria

- A rooted job that is leased but not admitted when the base moves
  is cancelled with no could-not-run strike, and its change is
  recomposed on the new base in the same run, with no STALE-BASE
  refusal validation for the old candidate.
- A rooted job whose donor has admitted it when the base moves is
  kept and decided as today.
- Records written before the admission field existed count as not
  admitted until a donor reports otherwise.

# Notes

From a consuming queue's backlog at 98f4b65d on 2026-11-07
(the 12:24 ET backlog entry). Owner `mu-mqwd` after
pre_claim_check.

Admission is reported just before the validator starts, after
tool materialization, identity refresh, fetch and checkout, so a
lease admitted and then held inside the validator is still
protected. Admission is opt-in per lease: the donor declares
`--reports-admission` at claim time, and a lease without the
declaration (including every record written before it existed)
keeps the old rule that a lease itself counts as started.

Decision on the kept-roots test: its cancelled branch encoded the
old lease-equals-started rule (`attempts == 0`). Under this issue a
job may be claimed and leased yet never admitted before the base
moves; cancelling it is the intended behaviour. The test now
accepts a cancelled first job only when it was never claimed or was
never admitted, and keeps the admitted branch (green at the old
base, requeued) unchanged.

## Resolution

Landed with the admission report described above: `Job::admitted`,
reported just before validator start, declared per lease with
`--reports-admission`; legacy leases keep the old rule. The
kept-roots integration test's cancelled branch was updated
deliberately: it encoded lease-equals-started, and now accepts a
cancellation only for a never-claimed or never-admitted job, with
the deterministic admitted/unadmitted branches asserted in the
companion kept-roots test.
