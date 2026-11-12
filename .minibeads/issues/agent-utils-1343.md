---
title: 'mq-compare-green-false-mismatch: compare-mode reads mismatch when both validations are green'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-11T19:15:18.000000000+00:00
updated_at: 2026-11-11T19:15:18.000000000+00:00
closed_at: 2026-11-11T19:15:18.000000000+00:00
---

# Description

A compare-mode job read `mismatch: local green, donor green` even though
both receipts were green: exit 0, not refused, every node passed. Both
receipts carried the same first-failure line — a passing step's verdict
line whose description mentions the gate outcomes it enforces
(`PASSED/FAILED/NO_RESULT`) — differing only in the step duration
(`(19s)` on one host, `(10s)` on the other). Because a full validation
runs that step every time, every compare-mode job read mismatch.

Two causes combine in `rs/merge-queue`:

- `Receipt::verdict()` fills `failure_line` from the receipt's carried
  lines (or the log tail) even when the run is green, and `compare()`
  decided match by whole-`Verdict` equality, so the duration text alone
  forced a mismatch. Normalising durations would not fix this: the line
  should never have been captured, and should never decide a compare.
- The failure-line capture matched `FAILED` as a substring anywhere in a
  line, so a passing step's description naming the outcomes it enforces
  was recorded as a failure line.

No Python edition of this capture exists (the queue is Rust-only), so no
parity change was needed.

# Design

- `compare()` in donor.rs decides on the outcome class
  (green / refused / red) plus the failed list. `failure_line` is quoted
  in a mismatch message as detail only: it never decides a match on its
  own, and it is ignored when both verdicts are green.
- `names_failure()` in validate.rs (behind `failure_lines()` and
  `first_failure_line()`, used wherever receipt failure lines are
  recorded) matches `FAIL`/`FAILED` only as a standalone status token —
  a whole whitespace-delimited word ignoring surrounding punctuation —
  and never captures a line carrying a `✓ PASS` marker. The remaining
  built-in patterns (`panicked at`, `error[`) and configured extra
  patterns are unchanged.

# Acceptance Criteria

- Two green verdicts whose failure lines differ only by duration, and
  mention `FAILED` only in a description, compare as `match`.
- A `✓ PASS` line with `FAILED` in its description is never captured by
  `names_failure`, `failure_lines`, or `first_failure_line`; `FAILED`
  glued to other words (`PASSED/FAILED/NO_RESULT`) is not a status token.
- `FAIL`/`FAILED` status-token lines, `panicked at`, and `error[` lines
  are still captured; existing red/refused compare cases are unchanged.

# Notes

## Resolution

[watermelon-full-preview] Fixed in donor.rs `compare()` (class plus
failed list; `failure_line` is message detail only) and validate.rs
`names_failure()` (status-token matching; `✓ PASS` lines never
captured). Regression tests:
`green_verdicts_match_when_failure_lines_differ_only_by_duration` and
`a_pass_line_with_failed_in_its_description_is_never_captured`, both
failing on the old code with the reported mismatch text and passing
after the fix. The merge-queue library suite passes.
