---
title: 'doc-link-bare-url: a bare GitHub markdown URL in a thread reply opens in document view'
status: open
priority: 0
issue_type: bug
assignee: mu-docread2
labels:
- vibe-talk
- documents
created_at: 2026-11-11T16:28:10+00:00
updated_at: 2026-11-11T16:28:10+00:00
---

# Description

[watermelon-full-preview] Owner reply 382 (fast-track): "These hyperlinks are GREAT... I wonder why clicking them doesn't automatically open in document view? I thought we had implemented that rewriting/hijacking for markdown github links? I thought I'd already seen this working somewhere else because clicking a document link opened in vibe talk but Command-clicking it still openned in chrome/github which is PERFECT."

Repro (Slack thread C0CC755QYSK, thread ts 1794413584.924419, reply message 1794413747.172589, on the vibe-talk devbig014 instance; read it from the live server or its cache read-only): in that reply, the parenthesized link https://github.com/rrnewton/agent-utils/blob/main/.minibeads/issues/agent-utils-833.md opened in the document reader, while the bare design link https://github.com/rrnewton/dev-agent-utils/blob/main/ai_docs/2026-11-03-vibe-talk-transcript-threads-design.md followed by a sentence-ending '.' did not. The server does mark dev-agent-utils links with data-doc (seen on an 11-10 link in the main list). Suspects, unproven: a bare URL with a trailing period (link detection or document-key canonicalization keeps the '.'), the thread pane not wiring the click handler, or the reader failing on the private repo.

# Design

- Find the real cause with that exact message before fixing: reproduce from the live server or its cache read-only, and prove which step drops the bare-URL case (detection, canonicalization, thread-pane click wiring, or reader fetch). Fix that step at the root; do not paper over the symptom in a second place.
- Keep Cmd/Ctrl-click opening the browser exactly as today; only a plain click opens document view.
- Add the smallest maintained test with the exact text shape: a bare GitHub markdown URL plus a sentence-ending '.', in a thread reply, alongside the parenthesized form that already works. No wall-clock budgets in tests (owner rule 346).
- Do not edit vibe-talk/web/voice.js without first naming the functions to the coordinator. Do not touch vibe-talk/web/src/view/documents/index.ts or list.ts without telling the coordinator first (mu-docfetch lane: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress); rebase over those landings if they move first.
- Owner fast-track: gate at owner tier with path selection and land alone; the coordinator deploys on landing.

# Acceptance Criteria

- The exact-shape test (bare URL plus '.', in a thread reply) fails before the fix and passes after it: a plain click opens the document reader, and the parenthesized form still opens it too.
- Cmd/Ctrl-click on the same links still opens the browser, at phone (393x852, touch) and desktop (1280x800) sizes.
- Existing document-link and reader checks stay green; the fix is gated at owner tier and landed on its own.

# Notes

Owner reply 382, fast-track. Filed by mu-docread2 before implementation; the fix lands alone as landing A. Siblings filed together: #1327 doc-reader-kind-header, #1328 doc-heading-size, #1330 doc-read-highlight.
