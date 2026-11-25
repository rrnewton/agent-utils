---
title: 'vibe-talk-identities-verified-wall-clock-test: message_cache_access::no_fill_begins_before_the_providers_account_is_checked waits identities_verified(200 ms) on the wall clock'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-25T10:55:00-05:00
updated_at: 2026-11-25T10:55:00-05:00
---

# Description

The integration test `message_cache_access::no_fill_begins_before_the_providers_account_is_checked` (vibe-talk/tests/message_cache_access.rs) holds the first provider's identity answer, then calls `identities_verified(Duration::from_millis(200))` and expects it to return false, and right after that asserts that the SECOND provider's verification is already `Verified`. The 200 ms wall-clock window is the only thing that lets the second provider's check finish before the assertion, so on a loaded host the assertion sees `Unverified` and the test fails (`left: Unverified, right: Verified` at the `verification(SECOND)` assertion).

It failed this way at host load around 143 in two lanes on 2026-11-25 and passes when run alone. Under the hot-machine rule (owner reply 346) a gating test waits on events or conditions, never on a wall-clock window.

# Design

- Keep the test's meaning: with the first provider's identity held, no fill begins for it, and the second provider verifies and fills.
- Replace the 200 ms window with condition waits: wait (with the suite's generous backstop) until the second provider is `Verified` and both identity calls were made, and check the first provider is still `Unverified` while its answer is held. If `identities_verified` must still be asserted false, call it only after the condition holds, or drive it with a paused clock.
- Look for the same pattern elsewhere in the file and in `src/cache/sink.rs` tests (`identities_verified(...)` with a short real timeout followed by state assertions) and fix those the same way.

# Acceptance Criteria

- The test passes with the host above 75% CPU (for example alongside a full gate) and still fails if the second provider is never verified or a fill begins for the held provider.
- No wall-clock window decides the outcome; every wait is on a condition with a backstop that only reports a hang.

# Notes

Found by the #2241 lane while running the full integration binary (1447 of 1448 passed; this was the one failure, in code that lane did not touch).
