---
title: 'vibe-talk-thread-card-reply-row-wrap: the thread card reply row wraps when it fits on one line'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T06:00:00-05:00
updated_at: 2026-11-23T06:00:00-05:00
---

# Description

On thread cards, the row with "about N replies", "M unread" and the newest-unread-reply preview box (#1906) line-wraps even when it would fit on one line. The owner also asked what the preview box is; that question (keep or drop the preview) is with the owner and is NOT part of this issue.

# Design

- Page-only: lay the row out so it stays on one line whenever its parts fit (no forced wrap, no fixed widths that overflow), and wraps cleanly only when the viewport is genuinely too narrow; the preview truncates with an ellipsis before the counts wrap.
- Do not remove or restyle the preview beyond what the layout needs.

# Acceptance Criteria

- Page test: at phone 412 and desktop 1280, a card with a typical count and preview renders the row on one line.
- Before and after shots: phone 412 and desktop 1280, light and dark.

# Notes

Owner UI request (fast-track); filed by the coordinator. The preview itself waits for the owner's keep or drop answer.
