---
title: 'merge-queue-pre-claim-check: a donor runs a configured per-candidate check before fetching a job, and gives it back when the check fails'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-07T14:45:00.000000000+00:00
updated_at: 2026-11-07T14:45:00.000000000+00:00
---

# Description

A donor can spend a job's setup only to discover the candidate cannot
run on this host at all (in a consuming queue, a pinned container
image that the host's cache no longer holds). The failure then reads
as a validation refusal, and the real reason is buried in the log
(see #1012 merge-queue-receipt-reasons). What the donor needs is a
generic gate before it takes the job: a configured check that
resolves whatever the candidate needs on this host, run per job,
before any fetch or checkout.

# Design

- `ValidatorConfig` gains an optional `pre_claim_check` (a shell
  command) and `pre_claim_timeout_secs` (0 means a 120-second
  default). Both are skipped in serialization when unset, so a
  config without them digests exactly as before; a config with them
  changes the validator digest, so queue host and donors must
  upgrade together in one window.
- A donor runs the check per job after the tool-identity and digest
  checks and before fetching the candidate, in its clone, with
  `MQ_CANDIDATE` (the candidate commit), `MQ_REPO` (the clone's
  origin URL) and `MQ_BASE` (the job's base commit) in the
  environment. The command resolves what it needs itself; the queue
  gains no concept of what is being checked.
- Exit 0 takes the job. Any other exit, or the timeout, gives the
  job back without declining it and without a refused receipt, with
  the donor's existing mismatch back-off; the first stderr line
  (cut at 200 chars, or "pre_claim_check timed out") is the reason
  shown in the release, the job history, and donor status.
- Absent config means no check, exactly as today.

# Acceptance Criteria

- With a passing check configured, a donor claims and validates a
  job as before.
- With a failing check, the job returns to the queue unclaimed by
  that donor: not declined, no receipt, no could-not-run record,
  the release reason naming the check's first stderr line, and the
  job offered again after the back-off.
- A check that overruns its timeout releases the job with the
  timeout reason in bounded time.
- With no check configured, claiming is unchanged.
- The check runs with `MQ_CANDIDATE`, `MQ_REPO` and `MQ_BASE` set
  to the job's commits and the clone's origin.

# Notes

Shape confirmed with the consuming queue's operator on 2026-11-07:
the checked resource is per-candidate, so no image or digest is
named in the config; the command resolves it from the candidate
itself. Consumers are named generically here, as everywhere in
this tree.
