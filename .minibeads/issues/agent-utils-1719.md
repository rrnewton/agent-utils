---
title: 'vibe-talk-h-key-fold-read: the h key does not toggle Fold read'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T14:56:37.875694791+00:00
updated_at: 2026-11-16T14:57:25.983302202+00:00
closed_at: 2026-11-16T14:57:25.983302072+00:00
---

# Description

[watermelon-full-preview] Owner report (fast-track, owner rule 286), 2026-11-16 9:50 AM ET: the h key does not toggle Fold read. Owner follow-up 9:51 AM ET: after refreshing the page, h works again. The keymap binds h -> readModeFromKeyboard (Show read and Fold read, in turn; controls todo-filter) in vibe-talk/web/src/view/shell/keymap.ts, acting in vibe-talk/web/src/view/shell/viewkeys.ts.

# Acceptance Criteria

Light check (coordb scope change, 2026-11-16): read the h binding, the dispatch gates and recent keymap history; a focused probe against the real dispatch and view-keys code for the focus/view states; close as not reproduced if no deterministic cause appears, noting the silent no-op states that match the report.
