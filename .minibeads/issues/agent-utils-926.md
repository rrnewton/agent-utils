---
title: 'agent-teams-not-delivered-state: a terminal ledger state for messages known not to have been delivered'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-06T05:50:00.000000000+00:00
updated_at: 2026-11-06T05:50:00.000000000+00:00
---

# Description

When a team message's target queue records it as failed for a known reason, the agent-teams ledger folds that into `possibly_submitted` ("may have arrived; needs a human"). One example is a prompt archived by `agentctl revive` (#899 revive-abandon-dead-candidate), which is known NOT to have been typed. That is safe, since it never claims delivery, but it is weaker than the truth: the sender is told to check, when the system already knows the message was not delivered.

# Design

Add a terminal ledger state, for example `not_delivered`, used only when the target's own record proves nothing was typed (a revive archive, or a refusal before any keystroke). Thread it through sender.rs's terminal and prunable sets, the outbox, receipts and the #904 delivery-outcome notice. Keep `possibly_submitted` for genuinely ambiguous cases.

# Acceptance Criteria

- A message archived by revive gives receipt state `not_delivered`, with the revive's reason as detail, and the sender's notice says it was not delivered.
- Ambiguous cases still give `possibly_submitted`; no path reports `delivered` without evidence.
- Older readers that don't know the new state treat it as not delivered, never as delivered.

# Notes

Found by cl-start while finishing #899 (2026-11-06 12:50 AM ET). Low priority, unassigned.
