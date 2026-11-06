---
title: 'voice-page-reply-parent-arrow: replies and quote-replies lose the blue jump-to-parent arrow, especially in threads'
status: closed
priority: 1
issue_type: bug
assignee: cl-refactor
created_at: 2026-11-06T13:15:00.000000000+00:00
updated_at: 2026-11-06T18:13:00.000000000+00:00
closed_at: 2026-11-06T18:13:00.000000000+00:00
---

# Description

OWNER report (reply 170, 2026-11-06 08:07 ET), verbatim: "When I reply or quote reply especially inside a thread. I'm not getting a little blue arrows to blink it back to the parent. I really like those. Even if reply is not a concept in slack, I thought that we were tracking it in our own metadata so that vibe talk could render them. Can we please get that fixed?"

The vibe-talk page used to show a blue arrow on a reply or quote-reply that jumps back to the parent message, driven by vibe-talk's own reply-to metadata. It is missing now, at least for replies inside a thread. It is likely a regression from #624 voice-page-modularity.

# Design

1. Reproduce reply and quote-reply, both in the main channel and inside a thread.
2. Find whether the reply-to metadata is not recorded for in-thread replies (backend or shim side), or the page no longer renders the arrow there. If it used to work, bisect across the #624 batches.
3. Restore the arrow in both contexts.

# Acceptance Criteria

- Reply and quote-reply show the blue jump-to-parent arrow in the main channel and inside a thread, and clicking it jumps to the parent.
- A regression test covers both contexts: a browser test if the page broke, a backend test if the metadata did.
- It lands between #624 batches (linearize rule) and ships with a vibe-talk deploy.

# Notes

Assigned to cl-refactor ahead of #624 batch 65, because that lane owns the page code during the refactor.

## Resolution

Landed as 7c3a284a and deployed with the next vibe-talk deploy. Slack keeps no reply pointer of its own, so the server now keeps one in two places: the op-key metadata payload, which carries reply_to and is read back with the post, and a durable store table, reply_pointers (schema v24, ids only, at most 50,000, purged with the rest), which is the source of truth because the metadata only survives in a 7-day cache. Covered by tests: a reply and a quote-reply, in the channel and inside a thread, end to end with and without metadata and across a restart.

Unverified: older replies are not backfilled. Replies posted before the deploy have no pointer and still show no arrow; a thread-root and quote-text backfill was offered to the owner as a follow-up.
