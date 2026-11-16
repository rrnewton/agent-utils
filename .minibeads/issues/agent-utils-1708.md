---
title: 'chat-links-utf8-panic: reply rendering panics on multi-byte text near a guard phrase'
status: open
priority: 0
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T12:40:06.000000000+00:00
updated_at: 2026-11-16T12:40:06.000000000+00:00
---

# Description

[watermelon-full-preview] P0 crash on the landed #1705 build (0fbb69ba): `chat reply` panicked at `rs/agentctl/src/chat_links.rs:569` with "end byte index 9 is not a char boundary; it is inside '–' (bytes 8..11)" on the text `(:issue:1234, parts 2b–3)`. `Renderer::render_guard_phrase` tests a guard phrase by slicing `rest[..phrase.len()]` by bytes before knowing the prefix is the ASCII phrase, so any reply whose text holds a multi-byte character (en/em dash, smart quote, emoji, CJK) straddling that byte offset crashes the reply path for every agent on the bridge.

# Design

Match guard phrases on bytes (`as_bytes().get(..n)` with `eq_ignore_ascii_case` on byte slices) and take every later span in the guard through `str::get`, so no slice in `render_guard_phrase` can panic. Audit every other byte-index slice in `chat_links.rs` (`render_span`, `render_tagged`, `strip_span_titles`, `parse_link_destination`, the backtick scanners): each is delimited by ASCII bytes at proven char boundaries and stays, with the guard as the only reachable panic. Nothing in reply rendering may panic on any input.

# Acceptance Criteria

- The reported text `(:issue:1234, parts 2b–3)` renders unchanged with no panic.
- En/em dashes, smart quotes, emoji and CJK placed right after and around each reference form (`:thread:`, `:email:`, `:issue:`) and each guard phrase (`thread NNN`, `reply NNN`, `per your NNNN`) render without panic, known references still link, and the guard still rewrites or reports as before.
- A property-style test over generated Unicode bodies asserts rendering never panics.

# Notes

Reported live on 0fbb69ba shortly after the #1705 deployment. Same rendering pass as #658 chat-bridge-permalinks.
