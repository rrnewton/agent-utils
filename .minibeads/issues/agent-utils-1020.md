---
title: 'agentctl-rebind-queue-target: a #987 rebind leaves the queue bound to the old target, so every send is refused'
status: closed
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T17:40:00.000000000+00:00
updated_at: 2026-11-07T21:29:21.000000000+00:00
closed_at: 2026-11-07T21:29:21.000000000+00:00
---

# Description

On 2026-11-07 a Muse lane's record pinned session A while its pane ran session B (`muse ... resume B`), so every `agentctl send` stayed pending with "custom harness \"muse\" is not the foreground process in pane". `agentctl anchor NAME --replace` refused ("the resumed session contradicts the recorded session; revive it instead"). The #987 rebind `agentctl anchor NAME --replace --session-id B --new-conversation` then succeeded: the record now names session B.

But the queue's `target.json` still says `{"kind": "pane", "pane_id": ...}`, while the record's target is now `{"kind": "session", "value": B}`. Every `agentctl send` and `agentctl drain` now refuses outright: "queue ... is bound to {kind: pane ...}, refusing different target {kind: session ...}". The lane went from pending-forever to refused, so it is still reachable only by typing into its pane. Two messages queued before the rebind, one of them from many hours earlier, sit in the inbox with stale content.

# Design

Root cause: the #987 rebind wrote the operator-asserted session into the record's routing session, so the record's delivery target became `{kind: session, value: B}`. agentctl's own rule is that an asserted session never pins routing, and Herdr reports no session for a Muse pane, so a session-bound target could never resolve, even with the queue rebound to it.

- `anchor --replace --session-id NEW --new-conversation` routes by the pane and its pinned process unless the pane itself reports the new session; only a reported session pins routing (then recorded `observed`, else `asserted`).
- When the record's target changes all the same (old routing session to pane, or to a reported new session), the rebind rebinds the queue's `target.json` under the queue's delivery and binding locks, binding first and record second, as a session follow does. Repeating the rebind accepts either binding, which also repairs a record left by the earlier rebind (record routed by session B, queue bound to the pane). The previous binding is kept in the record under `queue_rebinds` (last 8) and returned as `queue_rebound_from`.
- Prompts pending in the queue are listed with their age, and the rebind refuses before changing anything unless the operator passes `--deliver-pending` (keep them for the new conversation) or `--cancel-pending` (tombstone each one, untyped, into `cancelled/`; the Rust edition only, as only it has the cancel lane). The result lists each pending prompt and what was done with it.
- `agentctl status` reports `queue_binding_mismatch` (the queue's binding, the record's, the problem and the fix command) whenever the two disagree.

# Acceptance Criteria

- A test: after `anchor --replace --session-id NEW --new-conversation`, the queue binding names the record's target (the pane, when the pane does not report the session; the reported session otherwise), and repeating the rebind changes nothing.
- A test: a record left routed by the asserted session while its queue stays bound to the pane (the live case) is named by `agentctl status` and repaired by repeating the rebind.
- A test: pending prompts from before the rebind are neither delivered nor dropped silently: the rebind refuses and lists them with their age, leaving record, binding and queue unchanged; `--deliver-pending` keeps them; `--cancel-pending` sets them aside untyped.
- `agentctl status` names a record/queue target mismatch, with the fix.

# Notes

Found by the devbig030 coordinator right after deploying agentctl 888124cc, while rebinding a long-running Muse lane under #987 agentctl-muse-fresh-rebind. The lane's pending queue was left in place, not hand-edited.

## Resolution

[opus 5.5 cl-sync] Fixed in both editions as the Design above describes: the rebind routes by the
pane and its pinned process unless the pane reports the new session, rebinds the queue in the same
locked step when the target changes, holds pending prompts for `--deliver-pending` or
`--cancel-pending`, and `agentctl status` names a record/queue mismatch with the fix command.
Tests: four Rust tests in `rs/agentctl/src/subagents/tests/launch_spec.rs` (rebind and its repeat,
the live shape, pending prompts, a pane that reports the session) and two Python tests in
`py/tests/test_agentctl_launch_spec.py`.

Unverified live: the stuck lane's record is untouched. Its repair, once this build is deployed, is
`agentctl anchor NAME --replace --session-id <its native session> --new-conversation --cancel-pending`
(its two queued prompts are stale), run by the coordinator.
