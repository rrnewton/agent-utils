---
title: 'chat-subscription-slack-shared-deadline-race: the rejection deadline test intermittently reports a known outcome'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-07T21:20:00.000000000+00:00
updated_at: 2026-11-07T21:20:00.000000000+00:00
---

# Description

`rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper`
(`rs/chat-subscription-slack/tests/directory.rs:317`) intermittently
fails its assertion that a write begun under the shared deadline is
reported with an unknown outcome:

```
thread 'rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper' panicked at chat-subscription-slack/tests/directory.rs:317:5:
assert_eq!(response["error"]["outcome"], json!("unknown"))
```

Observed twice on 2026-11-07 on tree a39f8984 while rerunning gate
failures: once in a batch of four same-tree runs of the single test
(3 passed, 1 failed in 1.24 s) and once in a batch of eight (7
passed, 1 failed in 1.51 s). Passing runs finish in about 1.01 s.
The test's own deadline machinery is the subject: either the shared
deadline sometimes lets the slow write settle enough to know its
outcome, or the test's timing margin is too small under host load.

# Design

- Determine whether the deadline is actually shared between the
  lookup and the write in the failing interleaving, or whether the
  assertion races the write's completion report.
- If the product is correct, count the outcome from an event the
  test controls (the write's start/completion records) instead of a
  wall-clock margin, as with other load-sensitive tests.
- If the deadline is not shared in some path, fix that path and
  name it in the resolution.

# Acceptance Criteria

- The single test passes 10 of 10 same-tree runs under host load.
- The strengthened test still fails if the write is allowed to run
  past the shared deadline to a known outcome.
- The chat-subscription-slack suite remains green.

# Notes

Backlog item reported 2026-11-07; not to be fixed by the reporting
lane. Failing outputs are quoted in the description; do not treat a
single passing run as evidence either way.
