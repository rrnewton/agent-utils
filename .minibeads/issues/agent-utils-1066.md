---
title: 'merge-queue-early-red-receipt-cause: a donor receipt keeps the early-red failure lines, not only the log tail'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T14:49:20.000000000+00:00
updated_at: 2026-11-08T14:49:20.000000000+00:00
---

# Description

Same family as #1059 merge-queue-donor-job-refusal item (2) (cause over last line), for red runs instead of refusals. A donor receipt's `log_tail` misleads after an early red: in one reported batch the 36-line tail starts mid-line with the ids of tests in flight when fail-fast stopped the run (their label is cut off), then holds only ABORT lines; the real failure — a test that failed on every try, with its file and line — is not in the tail at all. Anyone reading the receipt or the queue record built from it sees teardown, not the failure.

# Design

Carry a structured early-red cause on the receipt in addition to the raw tail, mirroring the optional `refusal_cause` field #1059 added for refusals:

- The failing nodes' FAILED test ids, plus each node's first panic/assert line, picked by the donor from the full run log before the tail cut. Settle in the change what the results rows and the early-verdict plumbing already carry, so the receipt adds only the missing human-readable failure lines and never a second verdict.
- The queue records (and any operator-facing display built from the receipt) name that structured cause ahead of raw tail lines.
- The donor's tail cut starts on a line boundary, so a tail never opens mid-line with its label missing.
- Backward compatible like `refusal_cause`: an optional receipt field, absent on receipts written before it existed; old donors and old queues keep working in every combination, and a queue without the field keeps today's tail-only record.

# Acceptance Criteria

- A test: an early-red donor validation with a chatty teardown produces a receipt and queue record that name the FAILED test ids and each node's first panic/assert line, even when none of them survive in the raw log tail.
- A test: a donor receipt's log tail never starts mid-line.
- A test: a receipt without the new field (an old donor) is accepted and recorded exactly as today.
- A clean whole-crate merge-queue run (rust.merge-queue.test within its 2400 CPU-s budget).

# Notes

Reported 2026-11-08 by a consumer's queue operator via the lead coordinator, and split out by the #1059 merge-queue-donor-job-refusal lane the same day so the P1 stale-base fix was not delayed: this needs red-run failure extraction beyond the refusal path #1059 built. Related: #1059.
