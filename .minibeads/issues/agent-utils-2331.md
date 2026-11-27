---
title: 'chat-headless-reply-reminders: a headless target that ends a chat request''s turn without a reply gets one reminder, keyed on its event log'
status: closed
priority: 1
issue_type: bug
assignee: cl-leanprofile
labels:
- agentctl
- chat
- headless
created_at: 2026-11-26T20:00:00-05:00
updated_at: 2026-11-26T21:35:00-05:00
---

# Description

The chat bridge turns reply reminders off for headless targets: `chat_service.rs` resolves `settings.reply_reminders` to `None` when the target is headless, and the service's start log says "reply reminders and the stale composer guard are off". A chat request whose turn ends with no reply block and no `chat reply` command is never followed up.

On 2026-11-26 a headless coordinator started an owner request mid-turn in a long turn (18 minutes, five inputs started mid-turn). It answered the request before it in that turn with three reply blocks, all captured. For this one it wrote a status line saying it had replied, and never wrote the block. Its event log and its harness transcript hold no block for that reply id, so the bridge had nothing to capture. Nobody noticed for 45 minutes, until the owner asked.

Harvesting itself is sound: every assistant text event is parsed as it is appended, nothing is keyed to a turn, and a next turn starting at once does not pre-empt it. The gap is the missing reminder. Without it, a fenced reply that the model forgets is lost silently, so fences are not safe as the only reply path for a headless target.

# Design

Reminders for headless targets, keyed on the event log the bridge already follows instead of on idle guessing:

- When the input of a chat request completes (`input.completed` for its `chat-<key>` queue item, at `turn.completed`) or is cancelled after the model saw it, and the request then has no captured reply block and no command reply after a short grace (about 20 s), the bridge queues one reminder input to the agent.
  - Delivery is turn-boundary.
  - The reminder names the reply id, the message number and the request's sender and time, and gives the marker lines (and the reply command when offered).
- One reminder per request, through the existing reminder record, so a restart never repeats one. A reply captured later closes the request as usual.
- `chat status` (and the request listing) shows such a request as unanswered, with when the reminder went out.
- TUI targets keep their current reminder rules.

# Acceptance Criteria

- A test with a fake headless event log: a chat request whose input completes with no reply block gets exactly one reminder after the grace. A request answered by a block, or by a command, gets none. A bridge restart in between does not send a second one.
- `chat status` names the unanswered request.
- The start log no longer says reminders are off for headless targets.
- The harness support matrix row for chat-bridge delivery says headless targets get reminders.

# Notes

Found while tracing a lost reply for #2322 agentctl-coordinator-lean-headless-profile (owner requests 389/390), where fenced replies become the only reply path for a headless coordinator. Until this lands, a headless coordinator should check `agentctl chat sent NNN` for each request it answered in a long turn.

## Resolution

`rs/agentctl/src/chat_service/headless_reminders.rs` finds when each request's turn ended in the headless event log: the first `turn.completed` (or harness exit) after the latest start of its `chat-<key>` input, whether that input started alone, batched or mid-turn. Starts more than 10 minutes before the recorded delivery are ignored. `ReplyReminders::check_headless` runs every 5 s in the headless owner loop, after the capture tick, and reads the log at most every 10 s while requests wait. A request still holding no reply 20 s after its turn ended gets one reminder, recorded and composed with the existing `reply-reminders.json` machinery (once per request, restart-safe) and queued at the turn boundary through the headless queue. A refused queueing holds off the next reminder for 60 s. The start log no longer says reminders are off for headless targets. `chat status` gains `reply_reminders` (`unanswered_after_reminder`, `reminder_waiting`). The chat user guide and the harness support matrix row say so.

Tested:
- Unit tests over fake event logs: alone, batched and mid-turn starts; a dropped input; a harness exit; an open turn; the latest start counting; the grace; the read cache.
- A service test: one reminder after the turn ends with no block, none for a request answered by a captured block, none again on a second check or after a restart, and the status summary.
- Live on a scratch fake-provider bridge with a scratch headless agent (Claude Code 2.1.319, haiku). The agent answered a request with plain text, and its turn ended at 18:29:28 PT. The bridge queued the reminder at 18:29:52, the agent answered with the block at 18:29:53, and the bridge captured it and posted "reminded reply" through its outbound helper.
