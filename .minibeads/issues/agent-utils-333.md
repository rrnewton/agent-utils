---
title: 'page-live-reads-remainder: the shelved parts of the page''s live-read work'
status: open
priority: 3
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.627946597+00:00
updated_at: 2026-10-15T15:52:38.627946597+00:00
---

# Description

The 2026-10-13 #285 quick-wins-safe landing ported only part of the page's live-read work: the W7 rule (no second read of a view the server is still reading). The rest stays shelved and is preserved on GitHub as archived/vt/page-live-reads (13 commits): applying a live event's edit instead of re-reading, a fifteen-minute poll stretch, believing a read sent after a live event, re-reading on unsettled removals, and redrawing an open thread's heading when counts change. Main's read logic has changed a lot since (#308, #310, #315, #316), so this is a re-implementation, not a rebase. Decide which parts are still worth having and do them one at a time, or close this as superseded.
