---
title: 'server-side-search: a folded search_text and a detail=none trigram index'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- search
depends_on:
  agent-utils-521: parent-child
created_at: 2026-10-28T01:27:51.414020675+00:00
updated_at: 2026-10-28T01:27:51.414020675+00:00
---

# Description

[opus 5.5] Part of #521 server-side-search (design §10 recommendations 2 and 3). Want: search_text holds the page's haystack (author, drawn text with the page's line breaks, link addresses) folded with crate::search_query::fold, under a new SearchTextSource version the writer rebuilds in the background; message_fts moved to detail=none and queried with LIKE for folded terms of 3+ characters, every candidate re-matched in Rust; a schedule-safe migration of the live cache with the rebuild time and size stated.
