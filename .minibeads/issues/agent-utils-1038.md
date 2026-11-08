---
title: 'documents-outline-popup: replace the reader TOC with a pop-up outline and link the title to its source'
status: open
priority: 1
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T01:19:06.000000000+00:00
updated_at: 2026-11-08T01:19:06.000000000+00:00
---

# Description

Owner (thread 214, reply 213): the Documents reader's table of contents at the top is abysmal. GitHub's collapsible Outline is the visual reference: clickable sections in a bordered column, indented by level. The owner wants a pop-up for quick access rather than a pinned column that reflows the whole display, and when scrolled down the pop-up must clearly indicate where HERE is. The reader also has no hyperlink to the document at its original source; in the drilled-in reader the title should be that hyperlink.

Today `view/documents/reader.ts` draws the outline as a flat list of buttons at the top of the reader (`#doc-outline`) from the outline strings in `GET /api/v1/documents/{key}`, and `#doc-title` is plain text. Heading level is not carried to the client outline.

# Design

- A toggle button in the reader toolbar opens a bordered pop-up panel listing sections, indented by heading level, scrolling within itself; the document behind it does not reflow. A tap or Enter jumps to the heading and closes the panel. Heading levels are derived client-side from the rendered `h1`–`h6` elements in `#doc-body` (which `render.ts` already anchors with `documentHeadingId` ids) if clean; the server outline shape changes only if that proves unworkable.
- While the document scrolls, the panel marks the current section with a distinct font style (HERE). Keyboard: a documented toggle key (added to help), arrow keys to move in the panel, Enter to jump, Escape to close.
- The reader title becomes a link to the document's original URL (`document.url`), opening in a new tab with `rel="noopener noreferrer"`. The header bar stays slim; documents with no safe http(s) URL keep a plain-text title.
- Modules: `vibe-talk/web/src/view/documents/reader.ts`, `vibe-talk/web/voice.html`, `vibe-talk/web/styles/voice/21-documents.css`, `vibe-talk/web/src/view/dom/ids.ts` if new ids are needed. The documents list module is not touched.

# Acceptance Criteria

- Browser checks with real input at desktop and 393x852 (touch), against the real server documents API: the pop-up opens from the toggle and from its keyboard key, a section jump lands on the right heading, indentation matches heading levels, HERE marking follows scrolling, Escape/outside tap closes without reflowing the document.
- The title link carries the document's original URL, opens in a new tab, and has no opener; a document without a safe URL shows a plain title.
- The old top-of-reader TOC list is gone.

# Notes

[watermelon-full-preview] mu-docreader, thread 214 batch B. Sibling work: #1039 documents-read-parity, #1040 documents-voice-agent.
