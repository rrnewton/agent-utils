---
title: 'undo-key-general: one undo key for every undoable action'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-22T20:57:14.658544359+00:00
updated_at: 2026-10-23T01:36:43.338626584+00:00
closed_at: 2026-10-23T01:36:43.338626584+00:00
---

# Description

The owner: if we have an undo keystroke it should be a general undo action that applies to anything that we can undo and anything that pops up an undo chip. Today z undoes Done only; the bookmark-clear change added an 'Undo remove bookmark' chip that z ignores.

Build: z, and Ctrl+Z / Cmd+Z outside text fields (where it stays the browser's text undo), undoes the most recent undoable action that still has its chip up, whatever the kind (Done, bookmark removal, and any other undo chip in voice.js; inventory them). One undo path, newest first; the chip and the key always agree. Both keys listed in the ? help; the help text and the userguide say so.
