---
title: 'vibe-talk-wtf-channel-recent-reach: prove (or fix) how far the WTF channel_recent scan reaches in the cache-only walk'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-25T14:30:00-05:00
updated_at: 2026-11-25T14:30:00-05:00
---

# Description

Found by the before/after replay compare for #2232 vibe-talk-wtf-lookup-log-and-replay. A WTF lookup's channel_recent bucket asks the cache-only walk (`catchup_fetch`, Main view) for the pressed channel's newest 300 held messages and keeps the ones that name the selection's head phrase. In a synthetic fixture of about 370 top-level messages, filled page by page through the ordinary timeline read (Flat view, and again through Main), every lookup's log record shows `channel_recent_scanned: 99`: the walk returned the newest held page and stepped back nothing below it. Every hit in that compare therefore came from the global search bucket, and the channel_recent bucket's reach in a real deployment is unproven.

The walk steps back through `held_below`, which answers only from cache pages the file can prove whole (or their covered suffix). The fixture may simply never create such proofs, or the walk may not step back where it should for this caller. Either way the #2232 design's "newest 300" is not shown.

# Design

- Build a fixture whose older pages are proven the way a real page read proves them (the cache-first older reads the page uses), and assert how many messages the glossary's walk returns: 300 when the cache holds at least that many proven, fewer only when it holds fewer.
- If the walk stops at the newest page even with proven older pages, fix the call or the walk so the bucket reaches its window, still cache-only (no upstream read, no fill).
- Keep the lookup's existing budgets: the scan stays one cache read, never a provider call.

# Acceptance Criteria

- A through-router test with synthetic messages shows the channel_recent bucket scanning up to its 300-message window when the cache holds it (the lookup log's `channel_recent_scanned`), and finding a head-phrase definition older than the newest page without the global bucket.
- The read stays cache-only: the chat fake's read count does not move during the lookup.
- Tests use synthetic channels and terms only.

# Notes

Filed by the lane at the coordinator's request; a follow-up to #2232, not part of it.
