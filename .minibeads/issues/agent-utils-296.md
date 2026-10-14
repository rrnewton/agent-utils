---
title: 'copy-highlighted-message: Cmd-C with no text selected copies the highlighted message'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T13:51:56.926524371+00:00
updated_at: 2026-10-14T15:53:51.727461468+00:00
closed_at: 2026-10-14T15:53:51.727461348+00:00
---

# Description

Owner 2026-10-14: a keystroke to copy the highlighted (selected) message without making a text selection. Design agreed in chat: Cmd/Ctrl-C keeps its normal behaviour when the OS/browser has a non-empty text selection or the focus is in a text field; with no selection it copies the whole highlighted message (its full text, also when the row shows a summary or is elided) and says so briefly. Cmd-Shift-C is avoided because Chrome uses it for the DevTools element picker. A bare key (y, if free in KEYMAP) does the same outside text fields.
