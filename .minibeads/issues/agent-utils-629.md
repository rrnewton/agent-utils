---
title: 'documents-view-client: the Documents page (view button, single-column list, reader, navigator)'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- documents
depends_on:
  agent-utils-628: blocks
  agent-utils-624: blocks
created_at: 2026-10-28T13:42:40.600432644+00:00
updated_at: 2026-10-28T13:42:44.024463078+00:00
---

# Description

[muse watermelon] Client side of #544 documents-view-design as decided 2026-10-28 (ai_docs/transient/2026-10-26-documents-view/README.md). BLOCKED by #624 voice-page-modularity and by #628 documents-index-server. Must be written in the new module structure; do not add to the large page file.

Build:
- View button: tap cycles Voice -> Channel -> Documents; long press opens the picker; Ctrl+5 for Documents; ?v=docs and ?v=docs&d=<key> deep links.
- Single-column Documents list (not thread cards): search box, per-kind type-icon rows (GitHub Markdown, other GitHub files, pastes; Google Docs kind later), dimmed unfetched rows (URL/file name only), one tap fetches one, Fetch recent button at the bottom (most recent 10-20), fetched rows undimmed with real title and metadata. Default sort mention order, newest at bottom; list starts scrolled to the bottom.
- Reader: instant open from the server cache, async freshness check, New version available chip (tap to update, never swap under the reader), contents drawer, Linked in..., relative .md links in-app, Back.
- Read-aloud with the hierarchical navigator: focus+context row over the block row, max 2 rows on phone / 3 on desktop, peers compressed into ranges (6 ... 15), bottom-row section-boundary marks with labels (optional shading), numbered sections.
- Browser walks at 393x852 and desktop.

# Acceptance Criteria

Documents page works against the #628 documents-index-server APIs in the new module structure; view button cycles and opens the picker; Ctrl+5 opens Documents; list/fetch/freshness behaviours match the design; browser walks pass.
