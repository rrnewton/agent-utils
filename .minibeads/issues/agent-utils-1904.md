---
title: 'agentctl-send-to-manager-and-children: send to my manager or my children without naming them, and mark skip-level sends'
status: closed
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

## Resolution

[watermelon-full-preview] Landed e5fabbac on github/main (gated tree 62e97179, owner-priority gate pass; replayed over the moved main with the combined tree verified). `agentctl send --to-manager` and `send --to-children` resolve the recipient from the proved caller's `managed_by` (flags on `send`, not new subcommands, so `--file`, `--queue-only`, `--ttl`, `--message-id` and urgent delivery all keep working; an explicit `--message-id` is suffixed per child). With no manager, `--to-manager` delivers to the designated coordinator and the header says so; with neither it refuses, as it refuses an unproved caller. Skip-level sends (grandparent/grandchild) carry `skip-level: SENDER -> TARGET via CHAIN` in the existing header grammar, computed at send time with no new persisted field; direct, peer and unrelated sends stay plain. `list`/`status` (and every status-shaped row) gained `children_count` beside `managed_by` in both editions, which the cross-edition differential now covers. Verified by the three-level unit test (`relationship_sends_resolve_manager_children_and_skip_level`), the full agentctl Rust suite, and a real-binary scratch-registry run: lane -> lieutenant via `--to-manager`, lieutenant -> its two lanes only via `--to-children`, unmanaged caller -> coordinator with the fallback header, coordinator -> lane marked skip-level, direct sends plain, unproved caller refused (exit 75). The Python edition's plain `send` is unchanged; the relationship verbs are the Rust edition's, like `manage`/`tree`, and the matrix records that.
