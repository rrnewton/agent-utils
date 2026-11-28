---
title: 'chat-bridge-silent-losses: the Slack plugin can drop a rescued message without offering it, and the chat bridge drops closure batches past 64'
status: open
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- slack
- bridge
created_at: 2026-11-27T19:41:13-05:00
updated_at: 2026-11-27T19:41:13-05:00
---

# Description

The #2420 audit (owner decision 2026-11-27: the chat transport must never lose a message on a fragile check) found three places where the chat bridge path loses something silently.

- **S1, the Slack rescan:** a rescued message the bridge has not acknowledged, whose pending cursor matches neither the resumed cursor nor its prior one, is recorded as delivered and never offered again. Only a diagnostics line says so. Code: `chat-subscription-slack/src/plugin.rs`, the pending-rescue branch of opening the rescan (around line 953).
- **S2, the Slack rescan:** a queued rescued message that fails admission when it is offered is skipped silently, with no log and no retry (`plugin.rs` around line 1679).
- **C1, the chat bridge closure outbox** (#2338): when more than 64 closure batches are waiting to be told, the oldest batch is dropped and only counted. Its closed requests are never reported to the agent or the owner (`chat_runtime/reply_windows.rs`, the outbox cap, around line 624).

# Design

- S1: offer the pending rescued message again instead of recording it delivered. The bridge dedupes by message key, so an offer of a message it already admitted is harmless.
- S2: log the refusal with the message key and keep the message queued for the next pass, or record it as refused in the refusals file. It is never skipped silently.
- C1: never drop a closure batch. Merge the oldest batches into one, or raise the bound and alarm loudly, so every closed request is still told once.

# Acceptance Criteria

- S1: a test with a pending rescued message under an unrelated cursor shows it offered again, not marked delivered. It fails on main.
- S2: a test where the queued rescue fails admission shows a log line and the message still queued, or recorded as refused. It fails on main.
- C1: a test that fills the outbox past its bound shows every closed request still told, with no batch dropped. It fails on main.

# Notes

- Split from the #2420 audit (rows S1, S2 and C1) on 2026-11-27. The relay fixes stay under #2420.
- Related: #2338 (reply windows and the closure outbox), #2366 (watched threads).
