---
title: 'server-side-search: search the server cache and upstream, with a truthful searched-back-to horizon'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-25T22:26:28.482387583+00:00
updated_at: 2026-10-25T22:26:28.482387583+00:00
---

# Description

[opus 5.5] Owner, 2026-10-25 15:03 PDT: the search-older button on Slack's All view said "Nothing in All matches that search — all of it is loaded" over "112 messages from <channel A>", which looked wrong. The owner asked for (1) accurate counts and completeness claims; (2) the same query sent to the server, which answers from its message cache and, past that, from the chat service, catching the page up with just the far-back hits rather than contiguous history; (3) one search language (quoting, AND/OR/NOT) with exactly the same results on the page and the server; (4) a clear, monotone 'searched back to <date>' horizon across the page, the server cache and the upstream service; (5) a pilot of what Slack, Discord and Google Chat support upstream, then a design and implementation plan shared with the owner BEFORE implementing.

Pilot and plan: ai_docs/transient/2026-10-25-server-side-search/README.md. Nothing is implemented yet; the plan's numbered questions are open for the owner.
