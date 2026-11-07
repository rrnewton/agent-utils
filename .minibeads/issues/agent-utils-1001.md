---
title: 'merge-queue-early-verdict: stop a validation at the runner''s own early verdict, off by default'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-07T03:15:45.000000000+00:00
updated_at: 2026-11-07T03:15:45.000000000+00:00
---

# Description

A run holds a red candidate's slot until the runner exits, and a consuming
queue's runner may spend many minutes after its first final red on the rest of
its graph and on publishing its record. The early-red watch (`$MQ_EARLY_RED`)
stops a run only at a streamed failure in a step declared `RERUN-EXEMPT:`,
which such a runner does not stream. That runner can instead say, during the
run, that a red is final: it creates a file holding `FAILED <test>`.

Writing that file is the runner's decision alone: only it knows whether a
failure may still pass on its own retry, or will be classified as a refusal
(infrastructure, not the change). A queue that stopped at the first failure
line it saw would turn those into ejections.

# Design

- `validator.early_verdict` (off by default). With it on, a `command`
  runner gets `$MQ_EARLY_VERDICT`, the path of `early-verdict` in its run
  directory (removed before the run starts).
- The run's watch reads it alongside the streamed verdicts. A whole
  `FAILED <test>` line (ending in a newline) stops the run the way an early
  red does: results are the verdicts seen so far plus that test as failed,
  `early_red` is set, the slot is freed. It is not by itself a deterministic
  red, so `confirm_reruns` still confirms it.
- Anything else in the file, or a partial line, is ignored. With the option off
  the variable is never set and the file is never read.
- Independent of `disable_early_red`. Donors run the same code, so a donor's
  receipt arrives early too.

# Acceptance Criteria

- validate.rs unit tests: the verdict stops the run and fails the named test,
  keeping the streamed passes; it works with the early-red watch off; without
  the option the runner sees no variable and a file in the run directory is
  ignored; a partial line or other text never stops the run.
- Whole merge-queue suite, clippy -D warnings, cargo fmt --check, make validate.
- User guide: the option, the file contract and the runner's obligation.
