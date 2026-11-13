---
title: 'discuss-transcript-persist: the Discuss record is saved server-side; reopening never re-summarizes'
status: closed
priority: 0
issue_type: feature
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T19:00:56+00:00
updated_at: 2026-11-12T19:00:56+00:00
---

# Description

[watermelon-full-preview] Owner replies 413/009 (via coordb): the discussion pane is persistent — saved server-side per channel/thread, close/open is a no-op — but it captures a transient state, so the record is two-part: the TOP is the most recent summary; BELOW it the timeline of the discussion since.

Parent: #1316 catchup-side-agent. Follow-ups on this record: #1353 discuss-followup-composer. Staleness/Update design: #1388 discuss-summary-staleness-update (design only).

# Design

- One server-side record per channel/thread: `{current_summary: {scope, covered_ids, cited_ids, window, text/text_html, unknown_terms, model, stamp, created_at} | null, timeline: [...]}`. Covered and cited ids are kept (a later staleness banner computes from them; #1388, not built here).
- A new Summarize replaces `current_summary` and pushes the superseded one into the timeline as a collapsed entry. Follow-up turns (from #1353) append to the timeline.
- Open reads the record (GET) and draws summary + timeline; it never spends a model call. Phone and desktop see the same record.
- Clicking a message link (footer or inline citation) keeps the panel OPEN and jumps the list behind it; it neither closes nor re-summarizes.

# Acceptance Criteria

- Summarize once, close, reopen (same or other device profile): the same summary and timeline draw with zero new model calls and zero catch-up POSTs.
- A second Summarize supersedes: the old summary is a collapsed timeline entry under the new top summary.
- Message-link taps leave the panel open with its state intact while the list jumps behind.
- Server unit/integration tests cover record persistence, supersede, and reopen-without-model; browser walks cover phone 390x844 + desktop; gated at owner tier, landed on its own (with the #1388 design issue filed in the same landing).

# Notes

Owner replies 413, 009, 418. Number reserved by coordb. Storage is one new table + store trait methods in the three stores, beside the slice-1 contract.

## Resolution

Landed on main as addec444 (evening seams c2f8cab8, d8583e38, 60d805fb) in the evening batch e7207edb, deployed in vibe-talk release 286446d9, live 11-12 ~8:28 PM ET. One server-side record per channel/thread holds the current summary over its timeline; reopening draws it with no model call, a new Summarize collapses the superseded summary into the timeline, and message links keep the panel open.
