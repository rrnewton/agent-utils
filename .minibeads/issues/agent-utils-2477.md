---
title: 'slack-rejection-shared-deadline-race: rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper races a 1000 ms wall-clock deadline with fixed fake sleeps (not_applied instead of unknown on a busy host); quarantined until 2026-12-05, make the fake condition-driven'
status: open
priority: 1
issue_type: bug
labels:
- chat-subscription
- tests
- hot-host
created_at: 2026-11-28T12:15:00-05:00
updated_at: 2026-11-28T12:15:00-05:00
---

# Description

`rust.chat-subscription.slack` was red on 7 gate heads from 2026-11-22 to 2026-11-28, 3 of them in the last 48 hours. None of the 7 heads touched `rs/chat-subscription*`. In the step output kept for two of them, the failing case is the same:

    thread 'rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper' panicked at chat-subscription-slack/tests/directory.rs:317:5:
    assertion `left == right` failed
      left: String("not_applied")
     right: String("unknown")

The case (`rs/chat-subscription-slack/tests/directory.rs`) proves that the account lookup and the reaction write share one deadline. It sets these timings:

- the helper's deadline: `--timeout-ms=1000`;
- the fake client's `slack.user describe`: `sleep 0.45` (`author-slow`);
- the fake client's `slack.reaction add`: `sleep 0.65` (`reaction-slow`).

The design is that the write begins after the lookup and is killed at the shared deadline, so the outcome is `unknown`. On a busy host, starting the shell, the 0.45 s sleep and the process round trips can take the whole 1000 ms. Then the write never begins and the outcome is, correctly, `not_applied`. The product is right both times; the test depends on how fast the host runs a shell script (owner rule 346).

# Design

- **Interim (this change):** the case is under a dated load quarantine until 2026-12-05.
  - Until that date it returns at once and names this issue on stderr.
  - After the date it runs again by itself, the same rule as the Python suites' `load_quarantine(issue=..., expires=...)`.
  - `CHAT_SUBSCRIPTION_INCLUDE_QUARANTINED=1` runs it on demand.
- **Fix:** make the fake condition-driven, so "the write began before the deadline" is certain whatever the load. Two ways:
  - **A.** The fake's lookup blocks on a FIFO the test releases. Its write records that it began, then blocks until it is killed. The test releases the lookup once it has used a known share of a generous deadline (for example 3 s of 10 s). It then asserts that the write began, that the run ended at the shared deadline and not a fresh one after the write began, and that the outcome is `unknown`. Every margin is seconds wide, never a scheduling race.
  - **B.** Give the helper an injectable clock in tests, so the deadline is spent by the test's clock, not the host's.
- Check the sibling deadline cases in the same file (`rejection_persistence_time_is_part_of_the_same_cli_deadline`, `member_listing_has_one_deadline_for_all_pages`) for the same fixed-sleep race, and give them the same treatment if they have it.

# Acceptance Criteria

- The case asserts the shared deadline with a condition-driven fake (or an injected clock), and no fixed sleep decides its outcome.
- It passes 20 of 20 runs with host load1 at or above 237 (75% of 316 hardware threads), recorded with the load per run.
- The dated quarantine is removed in the same change, before 2026-12-05.

# Notes

Filed by `cl-redaudit` (number pre-assigned by the coordinator, 2026-11-28), from the frequent-red gate audit of 2026-11-28 (quarantine approved by the liaison).

- No issue named this case before.
- #827, #1025 and #1174 mention the shared deadline only as a feature.
