---
title: 'chat-bridge-never-halt: a full request store and a provider gap raise one alarm and keep the bridge admitting, never stall or halt'
status: open
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- slack
- bridge
created_at: 2026-11-27T23:12:00-05:00
updated_at: 2026-11-28T01:20:00-05:00
---

# Description

Step (c) of #2420 relay-never-lossy. The owner decided on 2026-11-27: no lossy transport; a check may warn but never drop or delay a message. The liaison ruled that D4, D5 and D6 alarm and keep delivering. The #2420 audit found three places where the chat bridge stops taking owner messages:

- **D4, chat run (agentctl):** when the request store is full (2,048 requests or 128 MiB), new owner messages stay unacknowledged until space frees up. Code: `chat_runtime.rs` (audit lines 71, 78) and `reply_windows.rs` around 485-525.
- **D5, chat run:** a provider gap or a boundary mismatch stalls the owner loop until an operator acts (`chat_runtime.rs` around 505-510, 1600-1669 and 8715-8829).
- **D6, the Slack plugin:** a GAP record, a stderr overflow or an unknown saved cursor halts the bridge (`chat-subscription-slack/src/cli.rs` around 259-271 and 321-334; `plugin.rs` around 804-812).

# Design

- D4: 2,048 becomes a soft cap. Reaching it raises ONE alarm at once and admission continues. The hard cap is 8,192. Above it, the oldest answered requests are retired first, and never an unanswered owner message.
- D5 and D6: ONE alarm at once, and the pump never halts. It resumes from the gap's far side and rescans the gap's window (Slack history), so messages inside the gap are admitted. The bridge's dedupe by message key absorbs any copy already admitted. Operator approval becomes optional (a person may still review the alarm).
- Persisted or wire changes, such as a gap-reconcile record, are told to coordb before the gate, for the other teams that read them.

# Acceptance Criteria

- D4: a test fills the store past 2,048 and sees the next owner message admitted plus one alarm. It fails on main.
- D5: a test with a provider gap sees the pump keep running, the gap's messages admitted by the rescan, and one alarm. It fails on main.
- D6: a test with a GAP record, a stderr overflow and an unknown cursor sees the bridge keep running, rescan, and alarm once. It fails on main.

# Notes

- Split from #2420 (audit rows D4, D5, D6) on 2026-11-27, numbered by coordb. Chat size limits are #2439.
- Boundary mismatch keeps its halt (coordb, 2026-11-28: a different batch at the current cursor can mean corruption, not just a gap); D5 covers provider gaps only. Follow-up proposal: when the batch offered at the current cursor differs from the saved boundary only by messages the bridge already holds under the same keys with the same content (a provider re-render), admit it as a boundary replacement with one alarm; any key with different content keeps the halt.
