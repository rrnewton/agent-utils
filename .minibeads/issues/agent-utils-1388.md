---
title: 'discuss-summary-staleness-update: banner and incremental Update for a saved summary (design)'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-12T19:00:56+00:00
updated_at: 2026-11-12T19:00:56+00:00
---

# Description

[watermelon-full-preview] Owner reply 009 (via coordb), design direction only: a saved Discuss summary goes stale as the channel moves. When the owner returns, the pane should say so and offer to refresh just the delta — not a fresh full summary.

Builds on the two-part record of #1345 discuss-transcript-persist (`current_summary {covered_ids, cited_ids, ...}` + timeline), which keeps the ids this design computes from. **Needs owner go before building the delta Update.**

# Design

- **Staleness banner (could ship early):** "X new unread · Y marked read since summary [Update]", computed deterministically server-side from the record's `covered_ids` against current read state and the cached window — no model call. X = unread now that the summary did not cover; Y = covered ids read since.
- **Incremental Update:** a new summary over only the delta: sections the owner has since read are removed, new sections added (drawn in green), the rest kept. Still cache-only (#1348) and one model call; the superseded summary collapses into the timeline as in #1345.
- **Owner alternatives to weigh at go time:** clear and start over (drop the record, fresh Summarize), or a timeline with past summaries minimized (the #1345 collapsed entries, perhaps the whole refresh model).

# Acceptance Criteria

- DESIGN ONLY in this issue: the banner counts, the delta context shape, and the keep/drop presentation are written down and the owner says go (or picks an alternative) before any of it is built.
- If built later: banner counts match a hand-computed fixture; Update covers exactly the delta unread; no full-window model spend on refresh.

# Notes

Owner reply 009. Number reserved by coordb; filed by mu-catchup inside the #1345 landing as instructed. Do not build the delta Update on this issue without owner go.
