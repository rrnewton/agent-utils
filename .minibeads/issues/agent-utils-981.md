---
title: 'chat-bridge-alarm-coalesce: coalesce repeating chat-bridge alarm notices in place'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T21:15:40.000000000+00:00
updated_at: 2026-11-06T21:15:40.000000000+00:00
---

# Description

chat-bridge-alarm-coalesce: coalesce repeating chat-bridge alarm notices in place

Owner report (reply 182, 16:09 ET). On one host a bridge's
"[chat-bridge WARNING]: 1 message to the coordinator undelivered for
5 min ... next reminder in 5 min" repeats as a new reply in the episode's
thread on a backoff (rs/agentctl/src/chat_service/owner_alarm.rs). The
module doc notes a later "replace message" operation could rewrite the
first warning in place; it was never built. The owner wants ONE notice
per episode that is updated in place, by editing it (Slack chat.update)
or by deleting the previous notice and posting the replacement, so the
channel never shows a growing stack of warnings.

Apply the same rule to every repeating bridge notice of this kind (owner
alarm, held/undelivered warnings, and the like).

Requirements:
1. Per episode (and per thread when the alarm belongs to a thread), at
   most one visible alarm message. A repeat replaces it (edit
   preferred; delete+repost if edit is not available for that
   transport) with the updated text: what is held, for how long, why,
   and the fix. When delivery resumes, update it to a short
   "resolved at HH:MM ET" line (or delete it; pick one and say why).
2. The message id is saved with the episode (owner-alarm.json) so a
   restarted bridge keeps replacing the same message. Forward
   compatible with existing state files (unknown keys kept).
3. The Slack plugin/outbound path must support update and delete; add
   the operation with tests on a fake transport. Other transports
   degrade to today's behaviour, documented.
4. Times in ET in the text. Keep `chat alarm ack` / `snooze` working.
5. Tests, full gate (make validate), then land on agent-utils main.

[opus 5] filed by mu-alarm from the owner's report.
