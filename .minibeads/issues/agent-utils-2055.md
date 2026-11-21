---
title: 'vibe-talk-server-panic-char-boundary: server panics slicing a string at byte 1 inside a multi-byte char'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- server
- panic
- unicode
created_at: 2026-11-21T10:35:00-05:00
updated_at: 2026-11-21T10:35:00-05:00
closed_at: 2026-11-21T10:35:00-05:00
---

# Description

[watermelon-full-preview] The deployed vibe-talk server panicked 7 times from
2026-11-21 12:23Z: "end byte index 1 is not a char boundary; it is inside '…'
(bytes 0..3 of string)" at core/src/str/mod.rs:846:21 on tokio-rt-worker.

Culprit: `speakable::internal_id` did `segment.split_at(1.min(segment.len()))`
on a URL path segment from a user-posted link on a configured internal host
(`say_address` -> `internal_spoken`). A segment starting with the 3-byte '…'
(a pasted, truncated link) panics. Reproduced locally with the identical
panic message, file and line on unfixed code.

Fix: `internal_id` now takes the first character (`chars().next()` +
`len_utf8`) instead of the first byte. The audit of every byte-index slice in
vibe-talk/src found all others use `find`/`char_indices`/`len_utf8` offsets or
ASCII guards; one more unsafe class is fixed in the same change:
`elevenlabs/mod.rs` truncated vendor body/reason text with a bare
`String::truncate(500)` (4 sites), which panics when byte 500 is inside a
multi-byte character — now via a `truncate_at_boundary` helper.

Regression coverage: a test feeding an internal link whose segment starts
with '…' (plus emoji/CJK/combining-mark property-style sweeps over random
UTF-8 strings for `internal_id`/`say_address` and the truncation helper)
never panics and always returns valid prefixes.
