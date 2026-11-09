---
title: 'merge-queue-keep-green-donor-receipt: a green donor receipt is lost when its run is killed'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T14:16:51.000000000+00:00
updated_at: 2026-11-09T14:16:51.000000000+00:00
---

# Description

Found 2026-11-09 by a consuming project's merge-queue lane (relayed through its
coordinator; consumer names redacted per repository convention), against the code at
dab2a70c. It is an improvement, not a correctness bug: today's behaviour is safe, it
just revalidates.

Background: `kept-validations.json` lets a *stopped* run's unused green batch
validations land in the next run without revalidation when the base is unchanged,
every change is still queued at its validated head, the rebuilt candidate has exactly
the validated tree, and the selection is the same (`Runner::land_kept`). A graceful
stop reaches `Runner::finish`, which writes that file. A SIGKILL does not.

What happened (2026-11-09): the queue host was killed (host failure, ~50 min of one
donor host's capacity lost). One donor job was green on the donor during the outage;
its receipt never reached the queue host, and in this code a receipt exists only in
the donor process's memory until the `donor report` route call succeeds — a failed
report discards it (`Unreported::Lost`). After the host returned, the job's lease
expired, the donor re-claimed and re-ran the same job, and the restarted run's
`Runner::new` sweep cancelled every still-live donor job as "superseded: the run
that published it ended without settling it", including that re-validation. The new
run rebuilt the tenant's batch from scratch on the unchanged base, at the same head.

Two independent losses compound: (1) a green receipt whose report fails is thrown
away on the donor, and (2) a new run cannot adopt a green result that *is* recorded
(a `Done` job from the killed run survives the sweep on disk but is never read as a
verdict again; `Runner::new` re-publishes every job under a fresh id).

# Design

Keep a green donor receipt across a killed run, in both halves, without weakening
any receipt check.

Donor side (new spool): when a `donor report` fails (host unreachable or run gone),
the donor keeps the receipt locally: persist `Report{token, receipt}` (mode 0600 —
it carries the lease token) under its work directory (e.g. `spool/<job-id>.json`)
instead of discarding it as `Unreported::Lost`, and retry the report when the host
answers. A donor holding a spooled receipt for a job never re-claims or re-runs that
job; the spool entry is deleted only when the queue accepts the report
(`{"accepted": true}`). Bounded: one entry per job, deleted on acceptance or when
the queue definitively refuses it.

Queue side (retain, re-verify, convert):

- Stamp published jobs with their batch identity, schema-additively
  (`#[serde(default)]`): the changes (PR + head at publish), the policy, and the
  publishing run id. Job files written before this carry no identity and stay
  ineligible, exactly as today. `Receipt` keeps its current shape; the base comes
  from the job record (`job.base`).
- `Runner::new`'s sweep retains eligible jobs instead of cancelling them all: a
  `Done` job with a green receipt, and a leased job that may still produce one, are
  retained under their original lease binding (donor, host, token) for a bounded
  retention (aligned with the existing 7-day `Done` prune); everything else is
  cancelled "superseded" exactly as today.
- A retained job accepts a late report by re-running the *identical* refusal chain
  as a live lease — schema, `receipt.job == job.id`, donor == retained-lease donor,
  token match, `receipt.commit == job.commit`, `receipt.tree == job.tree`, and the
  full tool check against the original job stamp — then marks `Done`. The live-lease
  check in `Store::report` is not relaxed globally; expired, cancelled-for-cause
  (stale/discarded/operator) and past-retention jobs stay refused.
- A later run then converts each retained `Done`, green, deciding (non-compare-only)
  receipt into a kept validation and runs it through `land_kept`'s existing checks
  verbatim: every change still queued (or landed in order) at the validated head,
  the candidate rebuilt on the *current* base has exactly the receipt's tree, and
  the selection matches. Match on tree, base and heads — never the candidate SHA:
  candidate SHAs are not reproducible across runs (the same base/head rebuilt under
  a different candidate SHA in the incident), and `land_kept` already treats the
  old tip only as its tree source.

Auth: no new trust is introduced. The route remains the host authentication, the
lease token remains the per-job credential (now hashed/retained rather than cleared
for retained jobs), and every receipt check that applies to a live lease applies
identically to a late one. What changes is *when* a report may arrive and *which*
green results a new run may adopt — never *what* a receipt must prove.

User guide: the donor/kept-validations sections gain the spool, the retention, and
the killed-run behaviour; the sweep's "superseded" record stays for jobs with no
green receipt.

# Acceptance Criteria

- Killed-run test (the write-up's, in cli_donor beside the existing
  killed-run-cancel case): publish a job, kill the queue run, have the donor
  report green after the process is gone, start a new run on the same base —
  the batch lands as a kept validation with **no second validation** of it
  (validator marker count stays at one), and a concurrent live job with no
  green receipt is still cancelled "superseded".
- A donor whose report fails keeps the receipt spooled and retries; it never
  re-claims or re-runs that job while spooled, and the spool is deleted on
  acceptance.
- A late report for a retained job with the wrong donor, wrong token, mismatched
  tree, or a cancelled-for-cause job is refused, exactly as a live lease refuses
  it today (donor.rs unit coverage).
- A retained job whose changes moved heads, whose rebuilt tree differs, or whose
  base moved falls back to ordinary planning/validation; its retained job is
  cancelled then.
- Existing donor, push-retry/kept-validation and run suites keep passing; the
  whole merge-queue crate stays clean.

# Notes

P2 improvement after #1127 in this lane's chain. The reporter's ~50 minutes is
one donor host's re-validation of one tenant batch, not a correctness loss.
Scope notes from the code mapping: neither `Job` nor `Receipt` carries PRs/heads
today (the batch identity exists only in the publishing run's memory), and
`kept-validations.json` is only written at graceful finish — both are addressed
in the Design rather than worked around.

## Resolution
