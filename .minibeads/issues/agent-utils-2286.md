---
title: 'vibe-talk-thread-listed-whole-test-wall-clock: a cache-serve thread test waits on real time and reds under a parallel run'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
- hot-machine
created_at: 2026-11-26T06:00:00-05:00
updated_at: 2026-11-26T06:00:00-05:00
---

# Description

`message_cache_serve::a_thread_listed_whole_since_its_count_is_complete_whatever_the_count_says` (`vibe-talk/tests/message_cache_serve.rs`, added in 19d6c70d) depends on wall-clock time, which a gating test must not (owner request 346, and the 2026-11-21 hot-machine rule: every normal build and test is designed from the start to pass on a hot machine, never timing-sensitive).

Evidence (2026-11-26, a #2279 tree on 241aaf58, this test's code untouched):

- In the full unit step run as validate runs it (`cd vibe-talk && ../scripts/with-node22 python3 ../scripts/case_timings.py run-cargo -- cargo test --release --locked --no-fail-fast -- --skip the_voice_page_suite_passes --exact`), it was the single red of 3553 cases: `tests/message_cache_serve.rs:83: every evaluation was counted`. That is the `settled()` helper's 20 s guard (`WAIT`) on `shadow_settled`.
- Run alone it passed 3 of 3 (`cargo test --release --locked --test integration -- message_cache_serve::a_thread_listed_whole_since`). Each run took 30.03 s: the test's rate-limited `conversations.replies` reads wait out real retry-after time, so most of its run is real seconds spent waiting.

A test that spends about 30 s of real time waiting, under a 20 s settle guard, passes or fails by host load, not by the code under test.

# Design

Remove the real-time dependency. In order of preference:

1. Inject the clock for the rate-limit waits. The test already drives a `ManualClock` for the cache; let the Slack fake's retry-after and the client's back-off wait on that clock, or on paused tokio time (`tokio::time::pause` and `advance`), so the waits take no real time.
2. Or wait on the condition itself: the shadow evaluation counted, the refresh landed. Keep a generous hang backstop only (no assertion that anything was fast).
3. If neither can land promptly, disable the test from the gating selection, with this issue as its tracker, until it does (owner 346: disable, then bring back when robust).

# Acceptance Criteria

- The test's run time no longer includes real retry-after waits: it finishes in well under a second of real time, apart from build time.
- It passes alone and inside the full parallel unit step on a hot host (load1 >= 237 for the whole run, recorded in the evidence), 5 runs each.
- No wall-clock budget is asserted; any remaining timeout is a hang backstop only.

# Notes

Filed by cl-labels for coordb (2026-11-26) after the #2279 rebased evidence. Related: the known wall-clock identity wait (#2245) in `message_cache_access::no_fill_begins_before_the_providers_account_is_checked` is the same class of problem.
