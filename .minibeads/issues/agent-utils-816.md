---
title: 'merge-queue-red-verdict-survives-probe-failure: a known red ejects even if the probe cannot run; every retry is bounded'
status: open
priority: 1
issue_type: bug
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T11:59:56.940228245+00:00
updated_at: 2026-11-03T11:59:56.940228245+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1), from a consumer team's live merge-queue run. A full validate of one candidate was genuinely RED (a real failing test). serve then ran a failed-tests probe, which was refused with exit 75 (COULD_NOT_RUN); serve DEFERRED instead of ejecting and started a second full validate of the same red SHA. A known red was turned into a retry loop by an unrelated probe failure.

# Design

1. A red full validate that names failed tests is a verdict: eject (or hold for the owner, per configuration) even if the follow-up probe cannot run. A probe failure only means culprit narrowing or the flaky check is skipped, and that is recorded; it never turns a known red into a retry.
2. Every retry path has a counted, configurable bound (default 3), after which it escalates to the owner with an alarm: infrastructure refusals (#814), probe failures, and donor could-not-run receipts. Bounded retry everywhere (owner rule).
3. Check #814's new infrastructure-refusal path does not create the same loop.

# Acceptance Criteria

- A red full validate followed by a probe that exits 75 ejects the candidate exactly once and records that narrowing was skipped.
- An infrastructure refusal retries at most N times (configurable), then stops and alarms the owner.
- A donor could-not-run receipt follows the same bound.
- Tests cover each of these.
