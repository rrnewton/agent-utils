---
title: 'discuss-footer-dense-links: the footer lists only the uncalled remainder, as HH:MM links'
status: open
priority: 0
issue_type: feature
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T16:47:30+00:00
updated_at: 2026-11-12T16:47:30+00:00
---

# Description

[watermelon-full-preview] Owner reply 418 (via coordb): every message the summary directly references is an inline "(HH:MM)" hyperlink in the answer (mu-discussq's #1350 server pass); the list at the end is "Other messages processed" — only the processed messages NOT explicitly called out — and Mark all targets all of them.

Parent: #1316 catchup-side-agent. Field agreement with mu-discussq (landed): `cited_ids` is the cited subset of `unread_ids` (first-citation order, deduped); `unread_ids`/`markable_ids`/`footer_links` stay all-processed.

# Design

- The panel footer is titled "Other messages processed" and lists `footer_links` minus `cited_ids`, each as a dense HH:MM-only link (no repeated author names). With no/empty cited set it lists everything, so it worked alone before #1350 and still does when nothing was cited.
- Mark read / Mark unread keep targeting `markable_ids` — all processed ids, cited and remainder alike.
- Link clicks keep jumping as slice 1 (and, with #1345, will keep the panel open).

# Acceptance Criteria

- A cited message appears inline in the answer and not in the footer; every uncited processed message appears once in the footer as an HH:MM link that jumps to it.
- With no citations the footer lists all processed messages; Mark read/unread send exactly all markable ids either way. Page tests pin remainder/cited/mark sets; walks pass at 390x844/393x852/1280x800; gated at owner tier, landed on its own.

# Notes

Owner reply 418. Number reserved by coordb. Server citation pass: #1350 discuss-summary-structure (mu-discussq, landed).
