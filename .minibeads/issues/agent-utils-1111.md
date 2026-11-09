---
title: 'merge-queue-root-base: a root planned after main moved mid-run is validated with the run start base as MQ_BASE'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T03:08:35.000000000+00:00
updated_at: 2026-11-09T03:08:35.000000000+00:00
---

# Description

Reported by a consuming project's merge-queue lane (2026-11-08,
relayed through its coordinator; names redacted per repository
convention). Their queue runs merge-queue on the queue host and
on every donor. One serve run can stay open while batches land.
When it takes in a new submission, it plans a new root batch on
the current tip of main. The validator gets `MQ_BASE` ("the
batch's base commit", validate.rs) and `MQ_PARENT` ("the
candidate's parent: the base for a root batch, the stacked-on
candidate for a stacked one"). That queue's validator adapter
uses `MQ_BASE` as its census base.

What happened (UTC, 2026-11-09):

- 01:59:05 run 20261109T015905Z starts on base
  fabd9416c2f6a5670beb227fcd8c76f8ae1e07e3.
- 02:42:17 a change lands as 4c70bb90e239a5715863a9c123b4fc2c9f103d5a.
  The run stays open, two other roots in flight.
- 02:54:07 the run applies a submission at
  6b94f15e789e93bc0b16c31301788cad84b5fedb, a fast-forward of
  4c70bb90.
- 02:54:10 event "1 batch(es) planned: <change> (root on
  4c70bb90e2); 0 held".
- 02:56:11 it validates as a disjoint root and is published as a
  donor job. Job record: base = fabd9416..., parent =
  4c70bb90....
- The donor validator then refuses on every attempt: no census
  is published for fabd9416, its supplied base.

Expected `MQ_BASE` = 4c70bb90 (the root's actual base, equal to
its parent). Actual `MQ_BASE` = fabd9416 (the run's base at
start). The donor's own base ref was already 4c70bb90.

Code (verified still true on current main):

- `run.rs`: `extra.push((BASE_ENV, self.job_base()))` puts the
  run's base into `MQ_BASE` for every job, and `publish()` records
  the same `job_base()` as the donor job's `base`; `job_base()`
  returns `report.base_start` once known. `MQ_PARENT` gets
  `job.parent`, which is correct.
- `donor.rs` documents `parent` as "the base itself for a root
  batch"; for a root planned after a landing in the same run the
  two differ, and the donor side sets `MQ_BASE` (validator env
  and selection) from the job record's `base`.
- The selection command is documented to diff HEAD relative to
  `$MQ_BASE`, so on the donor side a selection-limited policy
  diffs against the old base and includes the landed batch's
  changes. (Queue-side selection already passes the candidate's
  parent; that side is unchanged.)

Impact: census-enforcing changes refuse with exit 75 on every
attempt within the run (no census published for the old base),
so they re-queue forever inside the run; any base-sensitive
validator logic sees the wrong base. Workaround known to the
reporter: withdraw and resubmit in a fresh run. #1087 makes
mid-run roots more common (the late intake round plans on top
of a batch that may land), which raises exposure.

Reproduction: run with 2+ roots in flight; let one land; submit
a change that becomes a new root mid-run; the donor job has
base != parent and the validator gets `MQ_BASE` = the run's old
base.

# Design

Fix 1 from the report (not fix 2: deferring such a root to the
next run would undo what #1087 was for). Every job's `MQ_BASE`
is its own batch's real base, recorded at planning:

- `Candidate` and the internal `Job` gain a `base` field: the
  commit the batch's stack stands on — its parent for a root
  batch, the root's parent for a stacked one. The fill loop
  records it when it chooses the batch's parent (a parent that
  is a flight's tip inherits that flight's recorded base; a
  parent taken as the current base is its own base). Probe jobs
  inherit their candidate's base; the widen job's base is the
  base itself.
- `validate_one` hands the validator `MQ_BASE = job.base`, and
  `publish()` records `job.base` on the donor job, so the donor
  validator env and donor-side selection agree with the local
  path by construction. `job_base()` (the run-start answer)
  goes away; `report.base_start` stays what it is, a report
  field.
- Donor job files keep their existing `base` field and schema;
  jobs already published by an older queue keep the base they
  were stamped with. No protocol change: an older donor reads
  the same record, now with the right value in it.
- User guide: the `MQ_BASE`/`MQ_PARENT` description already
  promises "the batch's base commit"; state explicitly that a
  root planned mid-run is measured against the base it was
  planned on, not the run's starting base.

# Acceptance Criteria

- The reproduction: with 2+ roots in flight, one lands and a new
  mid-run root is planned on the new base. Its local validator
  sees `MQ_BASE` equal to its parent (the new base), its donor
  job record carries base == parent, and the selection command
  on the donor side is measured against the same base.
- A stacked batch keeps today's meaning: `MQ_BASE` is the base
  its stack stands on (the root's parent), `MQ_PARENT` the
  candidate beneath it — including for a stack planned mid-run
  on a mid-run root.
- Batches planned before any landing are unchanged (base ==
  run start base, as today).
- Existing disjoint-root, donor and intake suites keep passing;
  the whole merge-queue crate stays clean.

# Notes

P1 from a consuming project's queue, ahead of #1104/#1105 in
the intake chain and right after #1103. Landed-change evidence
run 20261109T015905Z in the description is from that queue's
state, read-only. Test homes: cli_disjoint_root (gated roots,
local validator env) plus cli_donor (job record), decided by
harness fit at implementation.

## Resolution
