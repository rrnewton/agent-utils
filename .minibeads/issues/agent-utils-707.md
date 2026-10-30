---
title: 'chat-prompt-anatomy: a one-line header, a fenced message, short commands that find their bridge'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-30T15:10:43.186330046+00:00
updated_at: 2026-10-30T15:10:43.186330046+00:00
---

# Description

[opus 5.5] Owner request 2026-10-30. The prompt a chat bridge types into its agent is a paragraph of metadata (Source, Sender, Thread), a long absolute snapshot path for `chat thread`, a 64-hex `--request` key and a `--bridge-state` path in the `chat reply` command, and a long backup-reply paragraph. Wanted:

1. One header line in `agentctl send`'s grammar: `[from <sender> via <provider> <channel> · sent: <time> · reply: <CHAT_REPLY_N>]`.
2. The message fenced between short tag lines, apart from header and footer.
3. A short footer: the reply markers, `agentctl chat thread N` for thread replies, and `If you have trouble, run: agentctl chat help N`; the long rules move to that help page and the userguide.
4. Bare `agentctl` commands (team PATH), short reply IDs, no `--bridge-state`/`--request`: agentctl finds the bridge that serves the calling agent (registry caller identity, or AGENTCTL_AGENT), with several bridges per workspace still working.
5. Bridge state lives with the project, `REGISTRY/bridges/<name>/`, with a migration (`agentctl chat migrate-state`) and the old locations still working.

Backward compatible throughout: `--bridge-state`, `--request`, `--reply-id` and `--thread` stay accepted, and commands printed by older prompts keep working.

Design note: ai_docs/transient/2026-10-30-chat-prompt-anatomy.md.
