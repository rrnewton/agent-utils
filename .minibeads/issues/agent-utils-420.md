---
title: 'bookmark-clear: an X on the bookmark''s line that takes it away, with Undo'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T13:44:39.504878077+00:00
updated_at: 2026-10-22T13:44:39.504878077+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "Right now if we place a bookmark I don't see any way to remove it. It should have a little X on the bookmark line." Removal existed (#240 reading-bookmark: Shift+B on the message just below the line, and the row's ⋯ or details sheet item Remove bookmark, both writing DELETE /api/v1/channels/:id/bookmark), but nothing on the line itself said so.

# Acceptance Criteria

A small X at the end of the bookmark's line in every list that draws it (Main, All, a thread, the right column), hit area at least 24px, aria-label and tooltip Remove bookmark. Pressing it removes the bookmark here and on the server, redraws without the line, and leaves the reader's position unchanged; it neither folds, selects nor reads the row. Every removal offers the page's short Undo chip for a few seconds, which puts the bookmark back (conditionally). Shift+B stays the keyboard removal and the ? help says so. Page tests (X removes it, server cleared, position does not jump, no X without a bookmark, Undo) and Chromium at 1280x800 and 393x852 with screenshots.
