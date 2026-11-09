---
title: 'merge-queue-stackgate-infra: infrastructure reds count in the stack gate window, so stacking stays off on a healthy change stream'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T10:44:05.000000000+00:00
updated_at: 2026-11-09T10:44:05.000000000+00:00
---

# Description

From a consuming project's lead (2026-11-09, relayed through
its coordinator; consumer names redacted per repository
convention). Live on the then-current main. One serve run
(20261109T073232Z) never opened root stacking, so every landing
turned the other greens stale (about one landing per 45 minutes
with three single-job donor hosts).

Root stacking requires `rooted0 && view.stack_allowed &&
root_stack_min_pass_percent > 0`; `stack_allowed` is the stack
gate (`stack_min_pass_percent`, 80 on that queue) over the last
20 decided batches, fixed at run start. At this run's start the
window was 10 green / 10 red = 50%, so stacking stayed off,
although per-head predicted pass was 0.76–0.85, above the root
threshold of 70. About 8 of the 10 reds are infrastructure: 4
setup-step fetch timeouts with 0 tests run (a cold compile
behind a build lock), plus artifact-checker timeouts; only
about 2 look like real change reds. The consumer is running
`stack_min_pass_percent = 50` as a stopgap until this ships.

What the code does with such a validation today (checked
against current main): the dagrun journal records the setup
step's `step_end` with `ok: false` and a budget `reason`
(`TIMEOUT >Ns`, `CPU-TIMEOUT ...`), downstream test steps skip
or never run, and `parse_journal` records the setup step as an
attributed `Fail` while discarding the journal's `reason`
field. `results.require(...)` then fills the selected steps in
as `Missing`. That shape is not an empty red
(`is_empty_red` needs no results, or an unattributed run whose
results are all `Error`/`Missing`), so even
`validator.empty_red = could-not-run` does not classify it; and
the validator prints no `MQ_COULD_NOT_RUN_SCOPE` line for its
own setup step timing out. The batch is decided red, lands in
the decision records as outcome `red`, and counts in
`recent_red_rate`'s window exactly like a change that broke
tests. The window itself already does the right thing with
genuine could-not-run batches: outcomes other than green/red
(`refused`, `cancelled`) are skipped, and the window reaches
back to the newest 20 decided (green/red) validations. The
whole defect is the classification.

# Design

Two parts; option (b) from the report (root stacking ignores
the stack gate) is explicitly not taken: the stack gate is the
queue-wide brake for a bad stretch and root stacking respects
it, and the per-head threshold alone is too weak a signal.

1. Classify correctly. A batch whose validation ran no tests
   because setup timed out or failed at a budget is
   could-not-run (the exit-75 class), not red, decided through
   the existing refused path with its reason kept:
   - Where the validator says so itself
     (`MQ_COULD_NOT_RUN_SCOPE` on a refusal), that already
     works today and does not change.
   - Queue-side rule for the dagrun validator when it does
     not: `parse_journal` learns the journal's `reason` field,
     and after `results.require(...)`, if the run is red, was
     not cancelled or already refused, no selected whole step
     has a `Pass` or `Fail` outcome (zero selected tests
     counted: every one is `Skip` or `Missing`), and every
     failed step in the journal carries an infrastructure
     reason (wall/CPU timeout or budget abort, OOM/memory
     cap, supervisor crash/vanish — the `step_failure_reason`
     vocabulary, never a plain exit code), the validation is a
     could-not-run refusal naming the first such step and its
     reason, with the scope line appended to the log as the
     bad-row path does so donor receipts classify it the same.
     A plain non-zero exit with zero counted stays red: a
     change that breaks the build still ejects, it does not
     defer. The command validator is unchanged (its empty reds
     already have `empty_red` and the scope contract).
     Refused batches keep the existing bounded path: requeued,
     counted against `max_deferrals`, escalated at the bound —
     never retried forever, never a strike against the change.
2. The stack gate window counts only change verdicts. This is
   already the behaviour once part 1 lands (`recent_red_rate`
   skips non-green/red outcomes before truncating to 20), so
   part 2 is tests pinning it at the gate's own level, plus
   the user guide saying so where `stack_min_pass_percent` is
   documented: could-not-run batches are skipped, not counted
   as red, and the window still reaches back 20 decided
   change batches.

Backward compatible: decision records already carry the
`refused` outcome, no schema changes; a queue whose setup
steps never time out sees no change. The classification is
not behind `validator.empty_red`: that setting keeps meaning
what it means for command validators, and a budget-killed
setup step is not a harness crash a queue opts into calling
red — no test verdict exists either way.

# Acceptance Criteria

- A dagrun validation whose setup step ends `TIMEOUT` (or
  CPU-timeout/OOM) with every selected step skipped or missing
  is decided could-not-run: refused, requeued within the
  deferral bound, its reason named in the run log/report, and
  its decision record's outcome is `refused`, not `red` —
  locally and on a donor receipt.
- The same shape with a plain non-zero setup exit (no budget
  reason) is still a red verdict, and a run where any selected
  test actually ran (pass or fail) is never reclassified.
- Window: decision history whose newest decided batches are
  16 green and 2 red, interleaved with 8 could-not-run
  refusals and followed by older reds, gives a recent pass
  rate of 80% over the newest 20 decided (stacking opens at a
  gate of 80); a stretch of real reds still closes it.
- Existing validate/decisions/run suites keep passing; the
  whole merge-queue crate stays clean.

# Notes

Consumer evidence: run 20261109T073232Z (stack gate 50% at run
start from ~8 infrastructure reds of 10; stopgap
`stack_min_pass_percent = 50` in place until this ships, to be
reverted to 80 by the consumer). After #1105 in the intake
chain. Test homes: validate.rs journal/classification unit
tests, decisions.rs window tests, and a run-level case if the
harness fits (decided at implementation).

## Resolution
