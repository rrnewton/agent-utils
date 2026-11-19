---
title: 'agentctl-send-to-manager-and-children: send to my manager or my children without naming them, and mark skip-level sends'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-19T00:00:00.000000000+00:00
updated_at: 2026-11-19T00:00:00.000000000+00:00
---

# Description

Owner request (verbatim): "Shouldn't agentctl have an explicit parent child relationship? Maybe this should have a different verb to make it easier. Not just arbitrary message to a name but also send manager. That provenance lead should directly be the parent of those 13 lanes. You can do skip level comm up or down but it should be rare. You can always peek at transcripts tho."

What already exists and is built on, not duplicated: agent records carry `created_by` (provenance, never changed) and `managed_by`; `agentctl start` records the calling agent as both; `agentctl manage NAME --by LT` re-parents (cycle refused, auditable); `agentctl tree` [--json] shows the hierarchy; watch-agents notices go to the nearest reachable manager up the chain, else the designated coordinator. What is missing is addressing by relationship: a lane that wants to tell its manager something must know and spell the manager's name, a lieutenant that wants to tell all its lanes must enumerate them, and nothing in a delivered prompt says when a send skipped a level.

# Design

Flags on the existing `send` verb, not new subcommands (`tell-manager` / `tell-children`): `send` already owns prompt reading (`--file`, positional text), the mandatory sender header, queueing (`--queue-only`, `--ttl`, `--supersede-key`, `--message-id`), urgent delivery and the headless/agentcloud/interactive branches, and a separate verb would have to re-implement or divergently subset all of it. Relationship addressing is a choice of recipient, not a new delivery mechanism, so it belongs where the recipient is named:

- `agentctl send --to-manager [--file F | TEXT]`: the calling agent, proved by process ancestry exactly as `manage` proves its caller (`caller_edge`: the registered agent whose pinned harness is an ancestor of this process), sends to its recorded `managed_by` without naming it. With no manager (no edge, or an edge no live record of this registry answers to), the prompt goes to the designated coordinator and the delivered header says so (`to-manager: none recorded; designated coordinator`). With neither, the send is refused and names both facts. An unproved caller is refused, as with `manage` without `--by`.
- `agentctl send --to-children [--file F | TEXT]`: one copy to each live agent (record lifecycle running or starting) whose `managed_by` resolves to the proved caller; durable queue, one record per child, via the same per-target delivery path as a named send. The result summary names who got it (`delivered` / per-child outcomes) and who did not; with no live children the send is refused rather than silently sending nothing. An explicit `--message-id` is suffixed per child (`ID-CHILD`) so each queue keeps its own idempotency key.
- Skip-level marking, computed at send time with no new persisted field: when the proved or named sender is a registered agent and the target is an ancestor of the sender but not its direct manager, or a descendant but not a direct child, the delivered header gains `skip-level: SENDER -> TARGET via INTERMEDIATE...` (the `managed_by` chain between them, e.g. `skip-level: lead-014 -> provlane-3 via provlead`). Direct manager and direct child sends stay plain; peer and unrelated sends stay plain, because the owner named skip-level up/down the chain as the rare case to make visible, and marking every unrelated send would make the mark meaningless. Header grammar is the existing one (`[from SENDER via ROUTE · KEY: VALUE]`), so older readers ignore the new field.
- `agentctl list` / `status`: rows already carry `managed_by`; add `children_count` (direct children in this registry, by resolved `managed_by`) so a per-lieutenant scan needs no tree walk.

Rust edition only: the Python edition has `send` but `manage`/`tree` are already Rust-only refusals there, and the hierarchy this addressing reads is the same machinery; the harness-support matrix records the new verbs as Rust-only.

# Acceptance Criteria

- On a scratch registry with a three-level tree (coordinator -> lieutenant -> lane): `send --to-manager` from the lane reaches the lieutenant; `send --to-children` from the lieutenant reaches its lanes only; a coordinator -> lane send is marked skip-level in the delivered header.
- `--to-manager` with no recorded manager delivers to the designated coordinator with the header saying so; with no coordinator either, it refuses.
- Direct manager/child sends and peer/unrelated sends carry no skip-level mark.
- `agentctl list` and `status` show `managed_by` and `children_count`.
- The `manage` help text mentions the new verbs; the agentctl skill and the harness-support matrix gain rows for them (Rust-only).
- No new persisted record field: skip-level and children counts are computed at send/list time.

# Notes

Owner priority. Reader skew: pinned older builds ignore the new header field by the existing header grammar, and no record field is added, so mixed builds keep reading each other's records.
