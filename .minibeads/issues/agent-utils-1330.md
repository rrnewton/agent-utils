---
title: 'doc-read-highlight: highlight the block being read in the document reader and keep it in view'
status: open
priority: 0
issue_type: feature
assignee: mu-docread2
labels:
- vibe-talk
- documents
created_at: 2026-11-11T16:28:10+00:00
updated_at: 2026-11-11T16:28:10+00:00
---

# Description

[watermelon-full-preview] Owner reply 385 (fast-track): "Oh the READ control on documents is actually working reasonably well... But it is NOT highlighting the text its reading like it does for messages. Why not?"

Since #1039 documents-read-parity (landed 5484c9bd and 85501d44), the reader reads documents through the shared read-aloud owner, with the scrubber, device/cloud toggle, and Next/Prev. But vibe-talk/web/src/view/documents/reader.ts has no playing-block highlight or follow-scroll, so during a document read nothing in the text shows where the voice is. The #1039 documents-read-parity resolution recorded this as an accepted v1 caveat ("no per-block light in the reader DOM"); this issue closes that caveat. Message reads already highlight the playing block and keep it in view.

# Design

- Highlight the block being read in the reader and keep it in view as the read advances, matching how messages do it: reuse the messages' highlight and follow mechanics (the shared read-aloud owner and its playing-block signals); do not fork them into a reader-only copy.
- Follow scrolling goes through the page's single scroll owner (#1251 scroll-single-owner) wherever that owner governs the reader surface; stopping or finishing a read clears the highlight, and a document read still never archives, dismisses, or marks any message read.
- Reader-side change (vibe-talk/web/src/view/documents/reader.ts plus shared hooks only as needed); no vibe-talk/web/voice.js edit without first naming the functions to the coordinator. Do not touch vibe-talk/web/src/view/documents/index.ts or list.ts without telling the coordinator first (mu-docfetch lane: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress).
- Browser test with real input and no wall-clock budgets (owner rule 346): start Read on a multi-block document and assert the highlight moves block to block and the highlighted block stays in view. Owner fast-track: gate at owner tier with path selection and land alone; the coordinator deploys on landing.

# Acceptance Criteria

- Starting Read on a multi-block document highlights the playing block, the highlight moves to the next block as the read advances (including via Next), and the highlighted block stays in view, at phone (393x852, touch) and desktop (1280x800) sizes.
- Stopping or finishing the read clears the highlight; message read-aloud highlight and follow behaviour are unchanged and their existing checks stay green.
- The fix is gated at owner tier and landed on its own.

# Notes

Owner reply 385, fast-track. Filed by mu-docread2 before implementation; the fix lands alone as landing C. Siblings filed together: #1326 doc-link-bare-url, #1327 doc-reader-kind-header, #1328 doc-heading-size.
