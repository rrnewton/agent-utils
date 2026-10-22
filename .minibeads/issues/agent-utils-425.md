---
title: 'collapsed-lines: let the reader choose how many lines a collapsed message shows'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-22T14:04:42.430576459+00:00
updated_at: 2026-10-22T14:04:42.430576459+00:00
---

# Description

The owner, 2026-10-22: one of the best features of vibe-talk compared to regular chat clients is the ability to expand/collapse message cards. By default collapsed cards are 3 lines tall. The default should stay 3 lines, but it should be adjustable to 1 line or 4 lines or whatever. This should be in Settings, plus a keystroke not likely to hit accidentally: Cmd-( and Cmd-) (Cmd+Shift+9, Cmd+Shift+0).

Build: a Settings control 'Collapsed message lines', 1-8, default 3, kept per device like neighbouring settings, applied everywhere collapsed rows are drawn via one CSS custom property. Keys Ctrl/Cmd+Shift+9 decrease and Ctrl/Cmd+Shift+0 increase, clamped 1-8, matched on event.key and by code, with a brief notice, never in text fields, never the browser zoom. Both listed in the ? help. The reader's position stays steady when the value changes.
