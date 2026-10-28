---
title: 'state-sync-model: the page works on local state and syncs revisioned, monotone state with the server (design)'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-28T13:48:38.206743506+00:00
updated_at: 2026-10-28T13:48:38.206743506+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 (threads 327, 351, 353): one common model for page/server communication. The app renders from local state; the server sends more or newer state by revision ('changes since N'), and more information only enriches the local view and never breaks it. Actions that need state the client lacks block only until that item syncs, within the same model and cached locally, never as one-off transient requests. The server is authoritative for messages; owner state edited on several devices (read marks, bookmarks, archive) uses a simple merge (last writer per item, or add-only sets). Phase 2: local persistence (IndexedDB plus a service worker) for instant start and offline reading of what was loaded; phase 3: an offline outbox (the two send ticks stay grey until the server has the message). New server APIs (documents #documents-index-server, cache-first reads #622) must be shaped to move onto this. Deliverable: a design doc under ai_docs/transient/ for owner review; implementation after #624 voice-page-modularity.
