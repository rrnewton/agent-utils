---
title: 'headless-reply-in-thinking: a headless coordinator wrote five reply blocks only in its thinking, so the bridge had nothing to capture; the request prompt never says a block in thinking is not sent'
status: open
priority: 1
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- headless
- bridge
created_at: 2026-11-27T10:25:00-05:00
updated_at: 2026-11-27T10:25:00-05:00
---

# Description

A consumer team's chat bridge (snapshot b3c45d01) serves a headless Claude Code coordinator. On 2026-11-27 the coordinator believed it had printed standalone reply blocks for 7606, 7605 (part 1), 7607, 7608 and 7609, each alone in its own assistant message with a tool call after it. The bridge captured none of them, and `chat sent` showed them missing.

- In the same turn the bridge did capture two blocks for 7605: the acknowledgement at 14:56:35Z and part 2 at 15:00:48Z.
- The coordinator noticed the gaps itself and sent 7606, part 1 of 7605, 7608 and 7609 again with `agentctl chat reply`. The owner waited 3 to 6 minutes for them.

Traced read-only from three sources: the session's Claude Code transcript, the wrapper's event log and the bridge log.

Cause: the five blocks were never ordinary text, so there was nothing to capture.

- Claude Code's own transcript holds, for both turns (14:52:21Z to 15:21Z), exactly four assistant text blocks: the two 7605 blocks the bridge captured, and the two turn-end summaries. No text block contains `<CHAT_REPLY_7606>`, `<CHAT_REPLY_7607>`, `<CHAT_REPLY_7608>`, `<CHAT_REPLY_7609>` or a first part of 7605.
- The wrapper's event log matches: turn 8368 has 3 `assistant.text` events and turn 8369 has 1.
- The other 49 assistant messages in those minutes hold only thinking and tool calls. Their thinking is stored empty, but with large signatures (up to 53 KB), and they report 431 to 15,849 output tokens (median 1,792) behind short shell commands. The tokens went into thinking.
- The few thinking summaries the transcript keeps read like the missing replies. At 14:59:02Z one carries the substance of the 7606 answer the coordinator later sent by command. At 15:00:36Z and 15:01:14Z two carry recap parts.
- So the coordinator composed its replies inside thinking. Those reached its own context, which is why it believed it had printed them, but not the output stream.

None of the suspected bridge faults applies:

- Harvest window, split message, dedupe and fence parsing: the bridge's transcript reader turns only `assistant.text` events into text (the wrapper drops thinking), and no such event held these blocks.
- The tool-call-after rule: the two captured 7605 blocks were also alone in their message, with a tool call right after, so a following tool call does not stop a capture.

This is the reply-in-thinking class already found in a Claude Code TUI coordinator (#391), whose answer was a reminder (#292). Two gaps remain:

- On this snapshot, a headless coordinator gets no reminder. #2331 adds a turn-end reminder for headless coordinators on main, but it landed after the snapshot.
- The request prompt itself never says a block written only in thinking is not sent. Only the reminder says so, after the fact.

#2331 also cannot catch a lost part of a multi-part reply: a request with one captured reply counts as answered, as 7605 did after its acknowledgement.

# Design

1. Prompt (prevention, every Claude Code target, pane or headless): the block footer gains one sentence, the same wording the reminder already uses: print the block as ordinary text, because a block written only in your thinking is not sent. Command-only prompts already say that nothing printed is sent, so they stay as they are.
2. Mid-turn notice for headless targets (detection while the turn still runs; behind a setting, on by default):
   - Once per request, when its prompt started mid-turn or at turn start, the transcript reader has since seen at least 3 tool calls over at least 2 minutes, and no reply for it is captured or stored, queue one short notice for delivery right away.
   - The notice says that no reply is captured yet, that a block in thinking is not sent, and gives the `chat reply` command.
   - A request already reminded, answered, closed or retired is never noticed. The notice is recorded before it is queued and taken back if it is never typed, like a reminder (#292).
   - Turn-end reminders (#2331) stay the backstop.
3. Diagnosis: the wrapper records one `assistant.message` event per assistant message: its block kinds (text, thinking, tool use) and output tokens, never thinking text.
   - `chat sent N` then adds, for a request with no reply, how many assistant messages since its prompt held only thinking and tool calls.
   - Traces like this one then need no raw transcript.
4. Deploy: the consumer bridge's next turnover should take a build with #2331. Until then, a missed reply surfaces only when the coordinator checks `chat sent` itself.

# Acceptance Criteria

- The fenced prompt footer says a block written only in thinking is not sent. A test pins the sentence and its absence from command-only prompts.
- A deterministic headless test (fake wrapper and event log):
  - A request whose turn goes on for 3 tool calls over 2 minutes with only thinking (no `assistant.text`) gets exactly one mid-turn notice.
  - A request answered first, or a turn that ends first, gets none.
  - A notice never typed is taken back and recorded once.
- `assistant.message` events carry block kinds and output tokens and never thinking text. `chat sent N` reports the thinking-only count for an unanswered request.
- The notice's thresholds and its setting are documented in the chat user guide.

# Notes

- Reported 2026-11-27 by the consumer team's lead through coordb; traced by cl-bridgeretire. The consumer's registry, event log, bridge state and logs were read only, and no chat command ran against them.
- Times are UTC.
- Turn 8368 ran from 14:52:21Z to 15:03:30Z, with an automatic compaction (967k to 31k tokens) at 14:54:42Z just before the replies. Turn 8369 ran from 15:03:30Z to 15:21:00Z and took 12 chat requests mid-turn.
- Related: #292 (reply-in-thinking reminder), #391 (the same cause in a TUI coordinator), #1974 (headless transcript from the event log), #2331 (headless turn-end reminders), #2338.
