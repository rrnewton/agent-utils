---
title: 'merge-queue-receipt-reasons: refusal receipts name the log tail instead of the refusal, and donor receipts drop the early-red flag'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-07T12:55:00.000000000+00:00
updated_at: 2026-11-07T13:05:00.000000000+00:00
---

# Description

Two receipt defects hide why a validation did not run or stopped early:

- When a validation could not run, the operator reason quoted the log's
  last line (`validate::cannot_execute`, and the donor receipt path in
  `Store::report`). A wrapper that logs cleanup after refusing
  (`validate-lock RELEASED`, `box FREE`) buries the actual refusal
  (`REFUSED: pinned image not present`), so the consumer's "pinned
  image not present" failure read as "box FREE" (consumer tracker
  dh-dv6ja).
- A donor runs the same early-red watches as the queue host, but its
  receipt carried no early-red flag, so queue-side `receipt_ran`
  hardcoded `early_red: false` for donor ejects. The queue therefore
  lost the early-red record and event for a donor validation and
  re-required tests the donor deliberately never reported (consumer
  tracker dh-4pn71, remaining half).

# Design

- Add `validate::refusal_line`: the first log line holding `REFUSED`,
  falling back to the last non-empty line, plus `cut_line` (200 chars).
  Both labelling sites use them, keeping the existing "last output:"
  wording.
- Add `early_red` to the donor `Receipt` (`#[serde(default)]`,
  skipped when false, so old receipts parse), set it from the donor's
  `Ran::early_red`, and use it in `receipt_ran`.
- Fix the user guide, which said both that donors produce no early
  red and that a donor's receipt arrives when its run stops.

# Acceptance Criteria

- A refusal log holding a `REFUSED` line before cleanup lines names
  that line in the could-not-run reason, locally and on a donor
  receipt; a log with no `REFUSED` line still names its last line.
- A receipt without the field parses as not early-red; a flagged
  receipt round-trips.
- A donor validation stopped by its runner's early verdict is
  recorded on the queue with `early_red: true` and ejects without
  waiting for the runner to finish.

# Notes

Reported from a consuming project's merge-queue operation on
2026-11-07 (consumer trackers dh-dv6ja and dh-4pn71). Consumers are
named generically here, as everywhere in this tree.

## Resolution

Landed with `validate::refusal_line` (first `REFUSED` line, falling
back to the last non-empty line) and `cut_line` used by both labelling
sites, and with `early_red` carried on the donor `Receipt` from the
donor's run into queue-side `receipt_ran`. The guide contradiction is
fixed. Covered by unit tests for the line choice and the receipt
round-trip, and by `cli_donor_early_verdict.rs` (a donor early verdict
recorded as an early red).
