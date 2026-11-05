---
title: 'chat-receipt-needs-evidence: no delivered receipt without positive delivery evidence, for any harness'
status: open
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T15:54:58.746428454+00:00
updated_at: 2026-11-05T15:54:58.746428454+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 10:53 AM ET), P0. The owner's words, verbatim: "…Is this agentterm TUI still running under 'unknown' harness type in the bridge? That's ok but I want to fix it to be better than this. Unknown should still never justify false-positive claim of delivery to the agent when nothing is delivered. That's the behavior I'm seeing now."

On a consumer team's bridge on this host (target: an agentcloud session viewed through agentterm, so the harness is unknown), the bridge shows a "delivered" receipt (the delivery reaction) when nothing reached the agent.

# Design

1. Reproduce the false receipt on that bridge: the delivery path, the evidence it used, and why the paste returning was treated as delivery.
2. One rule for every harness, unknown included: a receipt or "delivered" state requires positive delivery evidence. That means one of:
   - the prompt seen in the agent's input area;
   - the prompt in its transcript or session history (for an agentcloud session, the session history shows the message);
   - an explicit harness acknowledgement.

   Without evidence, the request is uncertain: it is retried under the existing Enter-under-uncertainty rules and alarmed per the backoff policy. A paste returning is never evidence.
3. Then first-class agentterm support (#852 parts B and D): recognition, plus idleness for reply reminders.

# Acceptance Criteria

- Test: an unknown harness with no delivery evidence gets no receipt, and the request is uncertain, retried and alarmed per policy.
- Test: when evidence appears later (for example, the session history shows the message), the receipt follows then.
- Test: for an agentcloud session, history-based evidence is used.
- The consumer team verifies the fix on its bridge.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 164). Assigned to cl-start, right after #891. Testing is coordinated with the consumer team through the lead.
