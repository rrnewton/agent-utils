---
title: 'vibe-talk-pipe-tables: render Markdown pipe tables in messages'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-28T17:39:22.136970710+00:00
updated_at: 2026-10-28T17:39:22.136970710+00:00
---

# Description

[opus 5.5] owner 11:52 ET. A GFM pipe table in a coordinator message (posted through the bridge) rendered in vibe-talk as variable-width jumbled text. Render pipe tables in message text as tables (or at least monospace) in every view; check what the bridge/shim actually posts (Slack mrkdwn has no tables; markdown-tables-render recovers only native rich_content tables). Quick fix may land before #624; otherwise after. #658 (owner 11:54 ET): prompts and reply markers use the full short id (CHAT_REPLY_B343) so agents and humans see the same token; prefix "B" (not thread-specific; does not clash with Meta D/P/T/S ids). Avoid two coiners racing on one counter: the bridge coins B-numbers for every request it receives (from native Slack or vibe-talk); vibe-talk displays them (from metadata), and only coins its own prefix (e.g. V) when no bridge is attached.
