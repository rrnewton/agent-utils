---
title: 'vibe-talk-short-refs-per-channel: short refs resolve per channel, never across bridges'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-19T14:55:37-05:00
updated_at: 2026-11-19T14:55:37-05:00
---

# Description

[watermelon-full-preview] Owner thread 226, 2026-11-19: a bare thread
number in a message did not link, and `:thread:6965` in a reply was not
turned into a hyperlink either. That thread lives in a second bridge's
`short-refs.json`, which the live instance does not list — and simply
listing it is unsafe: `vibe-talk/src/short_refs.rs` keys refs by number
alone (`HashMap<u64, ...>`, a later file wins on a duplicate number),
while the two bridges' message numbers overlap, so a `:msg:N` link
could resolve to the other bridge's message.

# Design

Key short refs by `(channel, n)`. A ref resolves using the channel
where it is rendered (the message's own channel, threaded through
`render::fill`, the live path and the search index), falling back to a
unique match across files only when the number is unambiguous; an
ambiguous ref with no channel match stays plain text (the existing
`shortref-unknown` class), never a wrong link. A later file still wins
on a duplicate `(channel, n)`. Sibling work: `#1565
vibe-talk-short-ref-links` (the original links), `#658
chat-bridge-permalinks` (bridge numbering).

# Acceptance Criteria

- Unit tests: two files with overlapping numbers in different channels
  each resolve correctly in their own channel; the ambiguous case
  stays plain; a unique number resolves from any channel.
- Server-side render test through the real router: `:thread:6965`
  rendered in its own channel's message links to that thread, and
  `:msg:5` renders per channel.
- Dynamic test on a test instance with evidence, plus the screenshot
  matrix, before landing.

# Notes

Lane mu-shortrefs/1915. The live overlay change (adding the second
bridge's file) is coordb's, outside this lane.

## Resolution
