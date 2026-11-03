---
title: 'chat-bridge-json-forward-compat: persisted agentctl files tolerate keys a newer version added'
status: closed
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
- chat
created_at: 2026-11-03T08:08:01.248521549+00:00
updated_at: 2026-11-03T10:11:12.482666692+00:00
closed_at: 2026-11-03T10:11:12.482666571+00:00
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

# Notes

## Resolution

[opus 5.5] A persisted file one agentctl writes and another reads no longer refuses a key a newer build added. The new `forward_compat` module (Rust) decodes a record, then names each key it does not know. A key a catch-all kept is named as it is read. A dropped key is found by comparing the document with the record written back out, skipping `null`, `false`, zero and empty values, which are what `skip_serializing_if` leaves out. Each file warns once per process and set of keys. No new dependency. Every chat state record goes through it (`read_document`). Duplicate keys and trailing data are still refused.

Kept: each struct below carries a `KeptKeys` catch-all, so an older build that rewrites the file keeps the key.
- `bridge.json` (already kept at the top since 377654fe): `OutboundCommandConfiguration`, `PromptLabels`, `RelayChannelConfig`. `chat init` still stores no unknown key at any level, because their values were never checked.
- Chat state records rewritten by more than one process (the service, and `chat reply` from perhaps another build): `RequestRecord`, `ReplyRecord`, and `RequestSettlement` inside a request (#784, which landed while this was in review).
- Agent records and their nested records, rewritten by revive, rename, adopt and anchor: `NameHistoryEntry`, `NativeSession`, `CloudRecord`, `CloudLaunch`, `Edge`, `Designation`, and revive `Journal` and `Proof`. These keep silently, as the top of an agent record already did. The two editions each write keys the other does not know, and every command reads every record, so a warning would fire on every command and differ between editions.
- `event-sinks.json` (`SinksFile`, `SinkConfig`, `Sender`, `Curl`) and `restore.json` entries (`AlsoRegistered`, `TimerWatch`, and `ScheduledUnit` from #743, which landed while this was in review): their catch-alls only detect keys, which the warning names by path. Neither file is rewritten from these structs: `chat init --from` copies `event-sinks.json` byte for byte, and `restore --write-agents` edits `restore.json` as a JSON value.
- `profiles.json` (`RawProfile`, plus `CloudLaunch` inside it): keys at the top, in a profile and in its `agentcloud` settings are named by path (`profiles.worker.surprise`) in one warning, then forgotten, so they never reach a launch or `agentctl profiles`. This replaces the earlier rule that a profile key was refused. A misspelt key is now a warning, not a refusal, which the guide says.

Ignored with a warning (`deny_unknown_fields` removed, no catch-all): these are records written by the service and read back, where losing a newer key at the next write costs nothing.
- `ReplyAliasRange`: it is `Copy`, two numbers.
- `Checkpoint`, `BoundaryMessageGuard`, `AdmissionRequestIntent`, `DeferredAdmissionGuard`, `AdmissionIntent`, `CommitReceipt`, `GapDiagnostic`, `CheckpointGapRetryRecord`, `BoundaryMismatch`, `BoundaryAcceptanceMessage`, `BoundaryAcceptanceRecord`.
- `RetiredReplyReceipt`, `LegacyRetirementRecordV1`, `RetirementReceiptChunk`, `RetirementRecord`, `RetiredRouteIndex`. The untagged `RetirementDocument` still tells the two shapes apart by their disjoint required fields.
- `FenceFeedbackRecord`, `PendingFenceFeedback`, `ReplyAliasRecord`, `ReplyAliasEntry`, `WithheldReply`, `ReplyBreakerRecord`, `ThreadEvent`, `SavedMessage`, `ProviderPayloadDocument`.
- `DeliveryAlarm`, `StalledRequest`, `ReceiptReactionLoss`, `LostReceiptReactions`, `ReceiptReactionRefusal`, `RefusalInFlight`, `RefusedReceiptReactions`, `ProviderHealthRecord`, `PathFailure`, `ServiceHealthRecord`, `AlarmSources`, `ReceiptReactionRecord`.
- `RejectionRecord`, `RejectionDocument`, `ReplyReminderRecord`, `RemindedRequest`, `PendingReminder`, `RepointRequest`, `UncertainRequest`, `RepointOutcome`, `TargetDocument`, `CredentialProblem`.
- `agent_queue::identity::ProcessIdentity` (in agent records and revive proofs), silently: an identity is compared for equality with one observed afresh, which a kept key would never match, so a build compares the fields it knows.

Kept strict, with the reason:
- `WireSuccess`, `WireFailure`, `ResolvedSenderWireReceipt`, `MembersWireReceipt`, `SendWireReceipt`, `ReactionWireReceipt`, `WireError`: replies from the outbound helper, a wire between processes of one build. They are not persisted.
- `SenderGrant`, `GrantDocument`: authority records. An older build that ignored a newer key narrowing a grant would grant more than meant, so refusing is the safe failure.
- `ProviderDown` (in `delivery-alarm-sources.json`): an absent field means that path is clear. A key this build cannot read must make the saved record untrusted, so the scan reads the provider record again, rather than read it as clear (3eb6cef3).
- `client.rs` `AgentStartupTimeout`, `AgentStartupTimeoutError`: they match one exact error Codex prints, and are not persisted.

Python edition:
- `profiles.json`: same rule and same warning text as Rust. The cross differential's `primary/profile/refusal/unknown-field` case became `primary/unknown-profile-key`, which both editions accept with one identical warning.
- Agent records: name history entries, `native_session`, the revive journal and proof, and the custom, foreign and harness process identities accept a superset of their keys. Name history, native session and journals keep the extras; identities ignore them.
- The chat configuration (`Config.parse`) warns about and ignores unknown top-level fields. Its `target` stays strict: those are identity assertions, and ignoring a newer one would deliver to a pane a newer build refuses.
- Left strict:
  - request records and reply items, which count each record's bytes against fixed budgets, so an unknown field is refused as unaccounted padding (CHAT_USER_GUIDE, existing tests);
  - command supervisor publications (a wire);
  - the frozen legacy embedded reply outbox;
  - inbound message frames (a wire).

Not covered here, and proposed as a follow-up: the versioned agent-record row schemas (`agentctl-session/v2`, `v3`, `agentctl-launch/v1`, `v2`) and the rename journal (`agentctl-rename/v1`). Both editions check them for their exact fields per schema version, by hand rather than with serde. Crates other than agentctl (merge-queue, wrkslotsd, gh-paced, wrkviz) are also out of scope.

Tests:
- `forward_compat` unit tests: dropped keys by path, defaults not named, kept keys named and written back.
- `a_bridge_with_every_optional_feature_off_writes_only_the_original_keys`: the byte-identical guard. Every optional field skips serializing when off.
- `subscription_environment_is_strict_bounded_and_backward_compatible`: a nested key in `outbound_command` is named, not stored by init, and a `labels` key survives a rewrite.
- `records_written_by_a_newer_agentctl_keep_its_keys_through_a_rewrite`: request and reply, through a real send.
- The delivery alarm test: top-level and nested keys read around.
- `keys_a_newer_agentctl_added_to_the_sinks_file_are_read_around`.
- The restore tests: nested timer and `also_registered` keys.
- `unknown_keys_are_ignored_with_a_warning_at_every_level` (profiles).
- `nested_keys_a_newer_agentctl_added_survive_a_rewrite`, in both editions (agent records).
- Python `test_config_reads_around_a_field_a_newer_agentctl_added`, plus updated profile, identity and revive tests.
- The cross differential pair.

The immediate remedy for the consumer stays what the Description says: move its pinned agentctl past 377654fe.
