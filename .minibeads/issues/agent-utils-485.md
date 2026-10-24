---
title: 'chat-file-messages: rescue Slack messages with files that the event stream never carries'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-10-24T10:42:01.750598649+00:00
updated_at: 2026-10-24T11:14:52.070453750+00:00
closed_at: 2026-10-24T11:14:52.070453400+00:00
---

# Description

[opus 5.5] The owner found that 9 of their Slack messages with screenshots were silently dropped by the agentctl chat bridge over four days. A message carrying a file never arrives as message.created on the event stream the Slack plugin tails, so neither the text nor the file reaches the agent. Owner: "It's surprising these get dropped completely rather than at least delivering the text. Can we at least rescue them on our periodic rescan, separate from the tailer?"

Findings (2026-10-24): the event source documents message.created as carrying file_ids, and no other slack event type carries messages (mention.created covers app mentions only), so there is nothing else to subscribe to. A file message posted today (ts 1792836302.768889) has no line in the plugin diagnostics log, while plain messages around it were delivered. The 9 file messages of the last five days were all top-level, all with no subtype.

Wanted: a periodic rescan in the Slack plugin, separate from the tail, that lists the channel's recent history and active threads with slack.message list and admits any allowed-sender message the bridge has not seen, through the same admission path as the stream, deduplicated by message ts across stream and rescan and across restarts, never re-admitting an edited message. The delivered prompt names attached file IDs with the exact command to download each to a temporary path. Bounded (page and thread limits, backoff on errors), logged, configurable interval, tests on the fake transport, docs in the chat userguide/quickstart and the agentctl-chat skill.

# Acceptance Criteria

A file message the stream never carries reaches the agent within one rescan interval, with its file IDs and download commands; a message seen by both stream and rescan is admitted once; restarts and edits never re-admit or stop the bridge; tests and docs cover it.
