---
title: 'message-cache-test-flakes: the message-cache integration tests fail intermittently on a busy host'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T09:35:55.346411929+00:00
updated_at: 2026-10-19T09:41:26.905636218+00:00
closed_at: 2026-10-19T09:41:26.905636108+00:00
---

# Description

[opus 5.5] The vibe-talk message-cache integration tests fail intermittently, on main after #297 cache-coverage-races landed as before it. They made main red at two pushes on 2026-10-17 and 2026-10-18, and they fail locally when a test binary is held to four CPUs (`taskset -c 0-3`): 5 of 30 runs of `message_cache_access` and 2 of 30 of `message_cache_fill`.

Seen so far, in four shapes:
- `message_cache_access`: a test closes its first deployment's cache and opens the same file again, and the open fails "message-cache.sqlite3 is already open in another process or another cache handle" (`a_deletion_reaches_the_file_before_the_account_is_checked_and_an_edit_marks_the_channel_suspect`, `a_threads_listing_never_vouches_for_a_main_message_whose_edit_was_left_out`; in CI and locally). The test's close drops its own handles, but something else still holds the cache, and with it the file's lock, for a moment.
- the shadow comparison of a by-id read is counted `late` instead of compared, so `content_differs` (or `agreed`) stays 0 (`message_cache_access` `a_threads_listing_never_vouches_for_a_main_message_whose_edit_was_left_out` in CI; `message_cache_fill` `a_short_flat_page_never_vouches_for_an_older_message_whose_pushed_edit_was_left_out` locally).
- `message_cache_access` `a_revoked_credential_erases_at_once_and_a_refusal_lasting_a_day_erases`: a provider whose credential is accepted again still reads `Suspended` where the test expects `Verified` (locally).
- `message_cache_fill` `a_deletion_made_while_the_server_was_down_is_never_vouched_for`: after a whole-channel read since the start, a by-id read is counted `stale_head` instead of served and agreed (CI, before #297).

Fix: find each cause, and make the tests wait for what they assert on (or the product stop holding what it should not), without loosening any assertion. Acceptance: each shape is explained in the closing commit, a deterministic test reproduces each one that is a product defect, and both binaries pass 100 runs in a row held to four CPUs.
