---
title: 'chat-bridge-duplicate-reply: a settled transcript reply is posted twice, and a fenceless turn earns no reminder'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T06:22:31.000000000+00:00
updated_at: 2026-11-08T06:46:31.000000000+00:00
closed_at: 2026-11-08T06:46:31.000000000+00:00
---

# Description

Owner-visible, on a co-ordinator bridge running this build's `agentctl chat` (`rs/agentctl/src/chat_runtime.rs`; the lines below are hands' read-only triage of 2026-11-07 and are the same on current main). Request 5955 was answered once and posted twice: reply 1 was captured from the co-ordinator's transcript and sent, and about 96 seconds later a second reply was captured from the pane and sent. It is not an outbound retry (`send_attempts` is 1). The second body is the same block as the screen drew it — flattened, one middle paragraph and the ending missing, every paragraph it has already in reply 1. The same shape appears 11 times on that bridge since 2026-11-04 and 4 times on another, each a transcript capture followed 60–170 seconds later by a pane capture of the same reply.

The mechanism, as triaged: after reply 1 is stored, its transcript block is settled (`settle_transcript_bodies`). Later scans read a partial screen copy of the block (scrolled or redrawn). `merge_transcript_blocks` does not pair it with its transcript block — `rendered_equivalent` fails on the missing text, and the length fallback needs the two within 3/4..4/3 while the copy measures about 0.73 of the source — so the copy carries no transcript text. In `capture_scanned_replies_locked` the identity check then misses, there is no clean-transcript entry, and `same_reply_rendered` accepts only a full rendered match rather than a paragraph-subset of a stored reply, so capture takes the screen-fallback path: held for `SCREEN_FALLBACK_AFTER` (60 s), then stored and posted as a new reply.

Second, from the same evening: requests answered in plain text with no reply fence — and no `chat reply` — got no reply reminder (no reply-reminders entry), though the reminder is the protection that turns a forgotten fence into a posted answer instead of silence the chat user cannot distinguish from being ignored. A recheck of that transcript confirmed every emitted fence (5955, 5957 twice, 5958) was captured within seconds; only the truly fenceless turn went nowhere, unreminded.

# Design

- In the Rust bridge's reply capture, a screen block may never become a second answer to a request that a settled transcript block already answered. Either drop an unpaired screen block whose paragraphs are a subset (subsequence) of a stored settled reply's rendered text, or never use the screen fallback for an identifier whose transcript block is settled; the implementation picks the more robust of the two against the capture code's own invariants and says why in the change. A genuinely second reply under the same request (a new block, not a rendered prefix or subset of the stored one) must still post.
- Find why no reply reminder fired when a turn ended with neither a fenced reply nor a `chat reply`, and fix that path so the reminder fires on that turn end. If the recheck's evidence shows the reminder logic is correct and the gap lies elsewhere (for example turn-end detection), fix the actual cause and say so.
- No change to what counts as a reply, to transcript capture of complete blocks, or to the screen fallback for requests the transcript never answered: bridges whose agents produce no readable transcript still depend on it.

# Acceptance Criteria

- Regression test, duplicate: a settled transcript reply followed by a partial screen copy missing one paragraph posts exactly once; the same at the observed roughly 0.73 length ratio posts exactly once. A second, distinct reply to the same request still posts.
- Regression test, reminder: a turn that ends with no fence and no `chat reply` produces a reply reminder; a turn that ends with either produces none.
- The existing chat test suite stays green, and the exact push tree passes the full validation gate, `check_client_names` and `cargo fmt --check`.

# Notes

[watermelon-full-preview] Filed by mu-spark from hands' read-only triage (build snapshot and current main share the cited lines). After this lands, the running bridges need a refresh to pick it up; that refresh is the co-ordinator's, not this change's.

## Resolution

[watermelon-full-preview] Fixed by mu-spark.

Duplicate re-post: capture now also recognises an unpaired screen block as a stored reply when every paragraph of it reads, in order, as a paragraph of that reply once rendered (the last perhaps cut short) — `rendered_paragraph_subset` beside `same_reply_rendered` in `rs/agentctl/src/chat_runtime.rs`, applied only to screen copies, never to transcript blocks. The alternative, never falling back to the screen for a settled identifier, was rejected: it would also drop a genuinely second screen-only reply under that ID while the transcript lags, and multi-reply requests are real. The subset rule drops only copies built solely from the stored reply's own paragraphs. Regression test `a_partial_screen_copy_of_a_settled_transcript_reply_is_not_stored_again` covers a copy missing its second paragraph and one missing its third at the observed length ratio — each stored a duplicate before, each posts exactly once now — and proves a distinct second reply under the same ID still posts past the fallback wait.

Missing reminder: the cause was the idle mapping, found live by hands. After a turn, Claude Code reports a session with background shell tasks still running as `shell`, not `idle` (observed turning `busy` to `shell` two seconds after turn end, never `idle` while persistent monitors run). `ClaudeAgents::state` counted everything but `idle` as busy, busy is not logged, and the idle period therefore never began for such a co-ordinator: no reminder could ever become due, however long it then sat at its prompt (the affected bridge's last reminded request predates the incident by days, with eligible fenceless turns since). `shell` now counts as idle — the turn is over; only background tasks run. The mapping test that asserted the old reading now asserts this one, and `a_turn_that_ends_without_a_fence_or_a_command_reply_is_reminded_once_it_is_idle` covers the incident sequence end to end: busy while the turn runs, exactly one reminder once idle, recorded reminded.

Follow-up, deliberately not in this change: `rs/agentctl/src/inbox/watch.rs` likewise maps only session `idle` authoritatively, with `shell` falling back to the Herdr state there. The impact differs (worker observation, not reminder gating) and it needs its own look before anyone aligns it.

Unverified here: the refreshed bridges in service (picking this up is the co-ordinator's refresh) and reminder timing on a saturated host, where the idle query itself can be slow; the shipped waits and retry holds are unchanged.
