---
title: 'draft-chip-honest: ignore empty drafts, really discard on confirm, show only when the composer is off screen'
status: closed
priority: 1
issue_type: bug
assignee: cl-refactor
created_at: 2026-11-04T19:00:42.756365822+00:00
updated_at: 2026-11-04T20:27:31.547954157+00:00
closed_at: 2026-11-04T20:27:31.547954017+00:00
---

# Description

Owner asks on the Draft chip (#819 voice-page-quoted-reply-and-draft-chip):
- Empty or whitespace-only drafts must never count.
- Cancel on a draft asks "Discard this draft?", and Yes really deletes it, from storage too, so the chip count drops.
- "Our draft button pops up even when the composer is on the screen. It should just be for jumping to it when off screen".

# Design

- The draft count ignores drafts whose text is empty after trimming, and such drafts are not kept in storage.
- Cancel opens a "Discard this draft?" confirmation. Yes removes the draft from the page state and from persistent storage, and the chip updates at once. No leaves it.
- The chip shows only when the composer holding a draft is off screen, as a jump to it. It hides while that composer is visible.

# Acceptance Criteria

- Page tests: whitespace drafts are not counted and not stored; Discard Yes/No; storage really cleared (survives reload); the chip is hidden with the composer on screen and shown when it is scrolled off.
- Gate passes.

# Notes

Owner asks lost while the coordinator was down (2026-11-03/04), relayed by the lead 2026-11-04. Before more #624 batches.
