---
title: 'settings-server-side: every setting outside Screen and keys is stored on the server and shared by all devices'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-23T14:37:51.126953072+00:00
updated_at: 2026-10-23T15:32:55.803634925+00:00
---

# Description

[muse watermelon] The owner: all settings should be persisted server-side except per-device ones like text size. Rule: the Screen and keys Settings section stays client-side (labelled '(client side)'); every other setting is stored on the server per user with per-key last-writer-wins, fetched at load, pushed on change, live-updated to other open devices, seeded once from local values without clobbering. Fits ai_docs/transient/2026-10-23-replication-model.md (two-party state, per-key LWW).
