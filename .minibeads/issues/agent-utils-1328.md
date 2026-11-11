---
title: 'doc-heading-size: bigger h1 to h3 headings in the document reader'
status: closed
priority: 0
issue_type: task
assignee: mu-docread2
labels:
- vibe-talk
- documents
created_at: 2026-11-11T16:28:10+00:00
updated_at: 2026-11-11T16:28:10+00:00
---

# Description

[watermelon-full-preview] Owner reply 384 (fast-track): "I also would like somewhat bigger headings fonts if that's easy to tweak."

Document reader headings (h1 to h3) currently read too close to body size to scan a long document by its outline, on phone and desktop alike.

# Design

- Make document reader h1 to h3 noticeably bigger on a sensible scale that keeps h1 above h2 above h3 above body text, at both phone and desktop widths.
- Reader styles only (the documents stylesheet under vibe-talk/web/styles/); no markup, outline, or reading behaviour change, and no vibe-talk/web/voice.js edit. Do not touch vibe-talk/web/src/view/documents/index.ts or list.ts without telling the coordinator first (mu-docfetch lane: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress).
- Check computed heading sizes in the browser at both sizes, waiting on conditions not wall-clock budgets (owner rule 346). Owner fast-track: gate at owner tier with path selection; lands together with #1327 doc-reader-kind-header as landing B, and the coordinator deploys on landing.

# Acceptance Criteria

- Reader h1, h2, and h3 computed font sizes are larger than before the change and keep their order (h1 largest), at phone (393x852, touch) and desktop (1280x800) sizes.
- Body text size and the documents list are unchanged; existing reader checks stay green.
- The change is gated at owner tier and landed together with #1327 doc-reader-kind-header.

# Notes

Owner reply 384, fast-track. Filed by mu-docread2 before implementation; lands with #1327 doc-reader-kind-header as landing B. Siblings filed together: #1326 doc-link-bare-url, #1327 doc-reader-kind-header, #1330 doc-read-highlight.

## Resolution

[watermelon-full-preview] Landed in 3c7eb1b1 with #1327 doc-reader-kind-header. Reader #doc-body h1/h2/h3 are now 1.8/1.5/1.3em at weight 650, line-height 1.25 (a document scale above the message column's 1.25/1.16/1.08em, whose weight and line-height they keep); before, reader h1 fell to the shell's global 1.1rem, below its own h2. Proof: documents_layout browser walk holds heading order and size floors at 393x852 and 1280x800; owner gate green, combined tree re-verified after rebase.
