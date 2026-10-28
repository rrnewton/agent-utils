---
title: 'upstream-search-deferred: search a provider''s own history, starting with Discord''s bot search API'
status: open
priority: 4
issue_type: task
labels:
- vibe-talk
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:55.656268178+00:00
updated_at: 2026-10-28T14:20:48.530614424+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-27). Deferred by choice: server-side search (#521 server-side-search) answers from our own cache. Later, per provider, add upstream search, starting with Discord's guild message search for bots (needs the message-content intent); Slack's classic search.messages is not usable on our path (#596 server-search-prep). File the first provider's slice when someone picks this up.
