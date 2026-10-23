---
title: 'replication-model: model what page, server and provider replicate, and how each piece merges'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- design
created_at: 2026-10-23T13:23:49.249733116+00:00
updated_at: 2026-10-23T17:06:33.837189000+00:00
closed_at: 2026-10-23T17:06:33.837189000+00:00
---

# Description

[muse watermelon] The owner, 2026-10-23: take stock of what state is replicated between the vibe-talk page, the vibe-talk server and the upstream chat provider, and model it formally (lattices, CRDT shapes, the Unison/Harmony lens of replicas, update detection and reconciliation) so optimistic client actions such as a reply that promotes its target into a thread can be reasoned about as monotonic local operations. Read/unread, Done, bookmarks and hues are private to page and server; message existence, content and thread membership involve the provider too.

# Design

ai_docs/transient/2026-10-23-replication-model.md: an inventory with code references, a formal model per class, the send lifecycle and reply-promotes-thread worked through, where today's code is sound or not with an ordered list of changes, a model-based test plan, and questions for the owner.

# Acceptance Criteria

The design document is landed; the changes it orders are filed or folded into the lanes doing them (#448 send-dedupe, reply-promotes-thread, slack-live-ingest).

# Notes

[opus 5.5] Closed: the design document landed and the owner answered its questions (2026-10-23). Implementation continues under its own issues: #458 replication-c9-lattice-fixes, #465 replication-c6-op-key-marker, #469 replication-model-tests and #470 replication-c8-root-count (C5 and C7 follow once #448 send-dedupe lands); C1 to C4 are the #448 send-dedupe lane.
