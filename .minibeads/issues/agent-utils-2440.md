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
updated_at: 2026-11-28T03:45:00-05:00
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
- 2026-11-28 (cl-bridgeretire), D4 close note:
  - D4 landed on main in the chat-stack landing d3494a71..23077655.
  - 4ad7631d: when the store is full and owner messages are held, the coordinator's pane gets ONE `[chat-bridge ALARM]` line at once, once per hold (queue ID `chat-capacity-hold-<held_since_millis>`), naming how to close finished requests (lead-014's ask).
  - a4046cbf (option 3; lead-014 and coordb: yes): at the 2,048 limit, after answered requests and unanswered ones past 24 h, any other unanswered request closes oldest first through the closure outbox, so an owner message is never held while one could make room. A closed OWNER request still awaiting a reply opens the agent's notice with an alarm line naming its key.
  - Tests: a_full_store_puts_one_alarm_line_in_the_agents_pane_at_once; a_closed_owner_request_awaiting_a_reply_gets_an_alarm_line_naming_its_key; the capacity admission test, rewritten so the next owner message is admitted at once and the oldest unanswered request is retired.
  - The request cap stays 2,048, deliberately, with no request_hard_cap key. More reply routes would push RouteCache::patterns past the 96 KiB herdr output-subscription budget and break reply capture. A higher cap needs a reworked matcher, which is a separate design.
  - D5/D6 landed as 23077655; cl-gapreconcile writes its close note.
- 2026-11-28 (cl-gapreconcile), D5/D6 close note:
  - D5/D6 landed on main as 23077655 (gated as 04516726), in the chat-stack landing d3494a71..23077655.
  - D5, chat run: a batch holding one gap event is admitted like a checkpoint at its far side, through the normal commit path, so the pump keeps running. gap.json keeps its keys and is written only as `resolved` for a provider gap; the new gap-reconcile.json (schema agentctl-chat-gap-reconcile/v1) records the window (last commit before the gap to the gap) and whether the agent and the owner were told. ONE pane line (queue ID `chat-gap-reconciled-<observed ms>`) and ONE owner-alarm notice per gap; `chat status` shows `gap_reconcile` for 24 h and `chat health` reports it DEGRADED until finished (agent told, provider alive 5 min past the gap).
  - Also in D5: a gap mixed with other events keeps the old halt; an older build's unresolved provider gap no longer blocks reconnect (the next batch resolves it and opens a reconcile; retry approvals are optional); a provider gap supersedes a rolled-back admission that waits for a replay the provider will never send.
  - D6, Slack plugin: a GAP record, a stderr overflow, an unknown saved cursor and an event behind a proven position are reported at the far side, a `slack1:since:` cursor taken when the gap was seen; the reason says whether the rescan backfills. The ledger's new `gap_cursor` makes a resume from that far side known and admits back to the ledger's start, so the gap's window is filled across a restart. Older plugins ignore `gap_cursor`.
  - Acceptance: D5 met by provider_gap_is_admitted_like_a_checkpoint_and_the_bridge_keeps_running and end_to_end_a_gap_is_admitted_alarmed_once_and_its_message_delivered_once (agentctl), plus a_provider_gap_puts_one_alarm_line_in_the_pane_and_one_owner_notice. D6 met by provider_gaps_and_unreadable_cursors_are_reported_at_their_far_side_and_the_stream_goes_on (GAP record, stderr overflow, unknown cursor), a_provider_gap_never_halts_the_bridge_and_the_rescan_fills_its_window_across_a_restart and an_owner_message_sent_inside_a_gap_is_delivered_exactly_once_across_restarts (lead-014's condition). Each fails on the code before it (the gap halted the bridge).
  - Status stays open. Remaining before close: (1) the rollout, with the Slack plugin and agentctl moving together from one snapshot (lead-014's coordinator bridge in daytime with their knowledge; ours in the maintenance window); a new plugin under an older agentctl loses that agentctl's 10-minute outage auto-resolve for gaps; (2) the live check in the maintenance window: the running bridge on that snapshot shows `gap_reconcile` (null when no gap occurred) in `chat status`, and `chat health` is not DEGRADED for a gap. Any #2439/#2440 status change waits for that check.
