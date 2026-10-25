---
title: 'pin-badge-lag: redraw the message badge optimistically'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-25T21:40:05.081908611+00:00
updated_at: 2026-10-25T21:47:31.105479974+00:00
closed_at: 2026-10-25T21:47:31.105479481+00:00
---

# Description

[GPT-6 Codex codex-vibe-discord] Reproduced pin badge lag on main-channel rows among thread cards: renderPins redresses channel rows but never these retained card-list rows. Reuse one drawRowPin implementation in both paths, including optimistic rollback. Decision: pinning both messages of a split/combined post is intended: one visible row represents all its constituents, and an existing combined-row regression explicitly requires both PUTs and both DELETEs. Preserve that behavior. New test holds PUT pending, checks immediate badge, refuses PUT and checks rollback; it fails on the original code.
