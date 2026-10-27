---
title: 'server-search-prep: question-independent groundwork for server-side search, and whether classic search.messages is usable'
status: closed
priority: 2
issue_type: task
assignee: cl-search0
created_at: 2026-10-27T21:30:21.164281906+00:00
updated_at: 2026-10-27T21:55:54.996319893+00:00
closed_at: 2026-10-27T21:55:54.996319783+00:00
---

# Description

[opus 5.5] Owner 2026-10-27 13:41: take preparatory steps toward #521 server-side-search that do not depend on the design questions, and find out definitively whether classic Slack search.messages can be called through our Slack access (even single-word search would give candidates for our matcher). Additions the same afternoon: a precedents section on the query syntax of Google web search, Gmail, Slack, Discord and Google Chat; settled answers (substring, case- and accent-insensitive, Gmail syntax plus from:/in:/before:/after:/has:); and an evaluation of existing crates (SQLite FTS5 trigram, tantivy, query-parser crates) before building the DSL, matcher or index. Design: ai_docs/transient/2026-10-25-server-side-search/README.md.
