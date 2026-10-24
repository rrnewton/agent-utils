---
title: 'storage-io-audit: audit vibe-talk''s disk storage and IO for compute and space efficiency'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T10:55:05.892404774+00:00
updated_at: 2026-10-24T10:55:05.892404774+00:00
---

# Description

[opus 5.5] The owner asked for an audit of the #471 replication-c5-post-keys send log and all other vibe-talk disk storage and IO: is it reasonably efficient in compute and disk space, and is there any reason not to use an efficient binary serialization?

Scope: the SQLite store (src/store, migrations through v21), the post_keys table, client_settings (#457), thread_hues (#427), voice turns, summaries, read marks; the message cache (src/cache: its own SQLite file, FTS5, coverage, the shadow); page-side storage (localStorage in web/voice.js); logs.

Deliverable: ai_docs/transient/2026-10-24-storage-io-audit.md with an inventory measured read-only on the deployments, hot-path syscall counts from a local run, a per-site serialization recommendation with numbers, and the inefficiencies found: low-risk ones fixed with before/after numbers, larger ones proposed.
