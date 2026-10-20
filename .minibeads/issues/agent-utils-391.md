---
title: 'fenced-reply-not-captured: a reply to a request typed mid-turn into a Claude Code coordinator was not captured'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T07:47:56.369629457+00:00
updated_at: 2026-10-20T07:53:17.944188268+00:00
closed_at: 2026-10-20T07:53:17.944188178+00:00
---

# Description

[opus 5.5] On 2026-10-20 at 00:54Z the bridge typed request 7dfe1806... into a Claude Code coordinator that was mid-turn (Claude Code queued it and surfaced it with the next tool result). The coordinator believed it printed a fenced reply under <CHAT_REPLY_001>, but the bridge captured nothing and the reply was sent only after agentctl chat reply and a service restart. Investigate why terminal capture missed it, with fixtures from a real Claude Code pane.

[opus 5.5] Cause, from the coordinator's Claude Code session transcript (read-only): no assistant text block of that session contains <CHAT_REPLY_001>. The prompt was enqueued at 00:54:20.858Z and surfaced mid-turn at 00:54:53Z ('The user sent a new message while you were working'). At 00:54:57Z the model produced the reply text ('Got your test message - the bridge was broken because...') only inside a thinking block, without marker lines, then went on with tool calls; at 00:55:59Z it said 'the bridge didn't pick up my reply from this pane' and used chat reply. So the pane never showed a marker block and capture was right to store nothing. Terminal capture of a block in real Claude Code rendering (a lead row then the block at a two-column indent, a wrapped body row, the turn's closing row with background shells, the input box with its no-break space, the footer) and of a reply that opens its own message after a mid-turn prompt echo is now covered by chat_runtime::tests::a_reply_in_a_live_claude_code_rendering_is_captured, built from a live pane read through Herdr on 2026-10-20. Not changed: the request prompt's wording. The idle reply reminder already says a reply written only in thinking is not delivered; a mid-turn prompt gets no such line until the agent goes idle. Whether the prompt itself should say 'print the reply as ordinary text, not in thinking' is left open.

# Acceptance Criteria

The cause is found and recorded; capture of a fenced reply in a realistic Claude Code rendering (bullet lead row, two-column indent, wrapped rows, mid-turn delivery) is covered by a fixture test.
