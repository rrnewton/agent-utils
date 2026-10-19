---
title: 'redraw-keeps-pane: a redraw must not move the keys to a pane the reader was not in'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T03:02:06.725677672+00:00
updated_at: 2026-10-19T09:55:21.162609561+00:00
closed_at: 2026-10-19T09:55:21.162609401+00:00
---

# Description

Found in review of #281 view-keys (present on main before it). In two columns, with a selection drawn in Main, click a card's replies button on the right: focus drops to <body> and lastPane is 'side'. A live Main message or an edit re-read then redraws Main, and keepSelection() gives focus to Main's redrawn row because focusLost() is true, so lastPane becomes 'main' and Esc no longer closes the right column's thread. Fix: when focus was lost, keepSelection should restore focus only within lastPane (or not at all), never hand it to the other pane. Page test plus a Chromium step: click a replies button, redraw Main, press Esc, the right column's thread closes.
