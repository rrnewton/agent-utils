---
title: 'merge-queue-validator-base-env: the validator run command does not receive the job base or parent'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-08T18:06:20.000000000+00:00
updated_at: 2026-11-08T22:37:24.000000000+00:00
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

## Resolution

Landed on main as 61a52c0efdc019233473277a428f325df3b75aa2 (gated commit 883bf49f, stacked on #1074's c05a286b in one gated push).

The validator run command now receives `MQ_BASE` (the batch's base
commit, full SHA) and `MQ_PARENT` (the candidate's parent: the base
for a root, the stacked-on candidate for a stacked batch), for local
and donor runs alike. Donor runs take both from the job record; the
record gained a `parent` field written at publish, and a job file
written before it existed still validates with the parent falling
back to the base. Both variables are documented in the user guide's
validator contract. Covered by cli_peripheral (local root and
stacked) and cli_donor (donor root and stacked, plus the old-job
fallback) tests. Gate: full shared phase on the gated stack (dagrun 235 passed / 3 failed; rust.merge-queue.test clean at 580 passed / 0 failed, profile wall 512.024s at user 507.908 + sys 711.855 CPU-s). The three failures were in untouched code (python.wrkslots.shard-4, vibe-talk.check.thread-card-header-browser, vibe-talk.check.voice-call-drops-browser) and each passed solo in an exclusive rerun (dagrun PASS, 5 passed / 0 failed in 223.8s). The gate-exclusive cross differentials were stopped under the coordinator waiver for this merge-queue-only diff. Main had moved one overlapping merge-queue commit (same-head resubmission plus plan-intake validator reload) since the gated base; the combined replay was patch-identical apart from that commit's publish-validator context line, with fmt, clippy -D warnings and the client-names check clean and cli_recount 19, cli_regen 31, cli_peripheral 2, cli_intake_window 6, cli_intake 8, cli_drain 5, cli_donor 38 and the lib suite 227 all passing on the combined tree, whose merge-queue paths are byte-identical to the landed tree.
