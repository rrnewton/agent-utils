---
title: 'chat-bridge-json-forward-compat: persisted agentctl files tolerate keys a newer version added'
status: open
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
- chat
created_at: 2026-11-03T08:08:01.248521549+00:00
updated_at: 2026-11-03T08:08:01.248521549+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1). #802 added `.config.delivery_latency_reaction` to bridge.json. An older agentctl that reads that file then fails hard ("invalid_frame: unknown field delivery_latency_reaction, expected one of ..."); a consumer's heartbeat publisher broke this way on 2026-11-03 when the key was turned on for its bridge.

Facts found while filing: since 377654fe (2026-10-29) the Rust BridgeConfiguration already keeps an unknown TOP-LEVEL key, warns once and ignores it; its nested objects stay strict. So the failing reader is a build older than 377654fe, which no change on main can fix: the immediate remedy is to move that consumer's pinned agentctl past 377654fe. What remains for main is to make sure no other state file has the same hazard.

deny_unknown_fields is still on structs in these Rust files (state and wire records among them): chat_runtime.rs (RequestRecord, ReplyRecord, Checkpoint, ReplyAliasRecord, ReplyReminderRecord, CommitReceipt, DeliveryAlarm, ReceiptReactionRecord, ...), chat_runtime/{repoint,reply_reminders,grants,rejections}.rs, chat_event_sink.rs (SinkConfig, SinksFile), subagents.rs and subagents/{revive,control,cloud}.rs (agent records: NameHistoryEntry, NativeSession, CloudLaunch, Designation), restore.rs, profiles.rs (RawProfile), credentials.rs, client.rs. The Python edition's bridge.json reader (py/agentctl/chat_runtime.py, chat_storage.py) has not been checked.

# Design

- Every PERSISTED file one agentctl version writes and another may read (bridge.json, request/reply/checkpoint records, agent.json and its nested records, restore.json, profiles.json, event-sinks.json) tolerates unknown keys: keep or ignore them, warn once, never refuse. Wire frames between processes of the same build may stay strict.
- Nested config objects get the same treatment as the top level when a newer version can add keys there.
- The Python edition's readers of the same files behave the same way.
- New keys are written only when on (skip_serializing_if), as good practice, so a default-off feature never touches older readers.
- List, in the issue, each struct changed and each one deliberately left strict with the reason.

# Acceptance Criteria

- A test per persisted file writes it with an extra unknown key (top level and nested) and the current build reads it with a warning, in both editions where both read it.
- A default-off feature leaves bridge.json byte-identical to before.
- The issue lists every deny_unknown_fields struct as changed or kept strict, with the reason.
