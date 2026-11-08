---
title: 'merge-queue-validator-base-env: the validator run command does not receive the job base or parent'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-08T18:06:20.000000000+00:00
updated_at: 2026-11-08T18:06:20.000000000+00:00
---

# Description

The validator `run` command's environment names the selection and
the result channels (`MQ_SELECTION`, `MQ_TESTS`, `MQ_TESTS_FILE`,
`MQ_RESULTS`, `MQ_HANDLE_FILE`, plus caller extras) but not the
commits the candidate was built from. `MQ_BASE` reaches the
`select` command and the regenerate command, but not `run`.

A consuming queue wants a queue-mode validator that checks the
job's own base (run only what the candidate changed, against the
base the candidate was actually built on). It cannot: the run
command is never told that base, and it cannot work it out
itself — a donor slot's own main refs can be arbitrarily stale
(observed weeks old), so a validator that diffs against its local
main diffs against the wrong tree. For a stacked batch the
meaningful parent is not even the base but the candidate it was
stacked on, which only the queue knows.

# Design

- Pass two more variables to the validator run command, for
  local and donor runs alike: `MQ_BASE` (the batch's base
  commit, full SHA) and `MQ_PARENT` (the candidate's parent:
  the base for a root, the stacked-on candidate for a stacked
  batch).
- On the donor path both come from the job record: it already
  carries `base`; add `parent` backward-compatibly (absent in
  older job files, where the donor falls back to the base). The
  publisher records the candidate's parent when the job is
  written.
- Additive only: validators that read neither variable are
  unaffected.
- Document both in the user guide's validator contract,
  alongside the existing run environment.

# Acceptance Criteria

- A local run's validator observes `MQ_BASE` == the batch base
  and `MQ_PARENT` == the candidate's parent, for a root (parent
  is the base) and for a stacked batch (parent is the
  stacked-on candidate).
- The same holds for a donor run, sourced from the job record;
  a job file written before `parent` existed still validates,
  with the parent falling back to the base.
- Existing validation, donor and stacking tests keep passing.

# Notes

Reported by a consuming project's merge queue (their queue-mode
validator needs the job's own base); consumers are referred to
generically here, per repository convention. Queued after the
recount follow-up lands, ahead of #1054 merge-queue-sync-exact.
Gate as a product change under the merge-queue rules (full shared
gate phase; exclusive phase waived for a merge-queue-only diff).
