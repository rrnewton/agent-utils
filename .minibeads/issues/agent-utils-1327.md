---
title: 'doc-reader-kind-header: the document reader header shows the document type and repo'
status: closed
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

[watermelon-full-preview] Owner reply 383 (fast-track): "once I'm in the document view I would still like some indication of the document type (minibeads issue) in the header bar. Otherwise I wasn't sure if I was looking at a github issue or minibeads issue or what."

Today the reader header does not say what kind of document is open, so a minibeads issue, a GitHub markdown file, and the other kinds look alike once the reader is open.

# Design

- The reader header shows the kind and, where it applies, the repo: "Minibeads issue <number> · <status> · <owner/repo>" for a .minibeads/issues file, "GitHub markdown · <owner/repo>" for other markdown, plus "GitHub file", "Gist", and "Paste" for those kinds.
- DocKind::GithubIssue is a .minibeads/issues file; label it as a minibeads issue (with its number and open/closed status), not as a GitHub issue.
- Page-only change in the document reader header (vibe-talk/web/src/view/documents/); no server behaviour change beyond fields the page already receives, and no vibe-talk/web/voice.js edit without first naming the functions to the coordinator. Do not touch vibe-talk/web/src/view/documents/index.ts or list.ts without telling the coordinator first (mu-docfetch lane: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress).
- Browser checks wait on conditions, not wall-clock budgets (owner rule 346). Owner fast-track: gate at owner tier with path selection; lands together with #1328 doc-heading-size as landing B, and the coordinator deploys on landing.

# Acceptance Criteria

- Opening each document kind shows its header label (minibeads issue with number, status, and repo; GitHub markdown with repo; GitHub file; Gist; Paste) at phone (393x852, touch) and desktop (1280x800) sizes.
- Existing reader checks stay green, and the header still fits at phone width without hiding the title or controls.
- The change is gated at owner tier and landed together with #1328 doc-heading-size.

# Notes

Owner reply 383, fast-track. Filed by mu-docread2 before implementation; lands with #1328 doc-heading-size as landing B. Siblings filed together: #1326 doc-link-bare-url, #1328 doc-heading-size, #1330 doc-read-highlight.

## Resolution

[watermelon-full-preview] Landed in 3c7eb1b1 with #1328 doc-heading-size. The reader header now names the open document's kind via pure documentKindLabel in reader.ts: Minibeads issue #<n> with status for github_issue (.minibeads/issues files, never "GitHub issue"), GitHub markdown, GitHub file, Gist, Paste, Google Doc, each with owner/repo only from a strict github.com URL; unknown kinds label nothing, and every open clears the label first so no stale kind survives. Proof: exhaustive documents.test.mjs cases plus documents_buckets and documents_layout browser walks at 393x852 and 1280x800; owner gate green, combined tree re-verified after rebase. Unverified in a browser walk: Gist, GitHub file, and Google Doc labels (unit-tested only; no existing walk seeds those kinds).
