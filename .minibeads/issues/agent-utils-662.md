---
title: 'chat-capture-from-transcript: capture coordinator replies from the harness transcript, not the screen'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T17:39:22.119722661+00:00
updated_at: 2026-10-28T17:39:22.119722661+00:00
---

# Description

[opus 5.5] owner 11:52 ET. CHAT_REPLY blocks printed mid-turn (followed by a tool call in the same message) are not captured: Claude Code's pane shows a one-line summary in place of mid-turn assistant text, so the screen scraper never sees the block complete (lost 2026-10-26 237-241, 2026-10-28 365/377/387; bridge then reminds). Fix: capture reply blocks from the harness transcript (Claude Code / Codex / Muse session JSONL, cf. #574 harness adapters) instead of the screen, keeping the screen as fallback; count capture failures as log events.
