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
- Reply path (owner reply 154 policy, folded in 2026-11-05): when the bridge follows NO transcript for a delivery (unknown or agentterm harness, headless, transcript not found), the request prompt gives ONLY "reply with `agentctl chat reply N --file FILE`, then `chat sent`", and no reply-block (fence) instructions. Fences are offered only where a transcript is followed. This is decided per delivery, including right after a `chat repoint`, and recorded in `chat status`. Test: a repoint from an agentcloud target to an unknown-harness pane switches the next prompt to command-only.
- From the owner's reply-154 spec: where a transcript IS followed (Claude, Codex or Muse with the transcript found), today's prompt stays (fences plus the reply command). Fence capture stays as a fallback, so fences seen on screen are still accepted, but they are never asked for in the no-transcript case. The prompt's mode is recorded in its delivery record as well as in `chat status`. Tests: the unknown harness gets the command-only prompt; Claude with a transcript gets the fence prompt; a transcript lost mid-session switches the mode. Docs: the agentctl-chat skill and the user guide.
- An agentterm pane attached to an agentcloud session captures replies from that session's agentcloudctl history, the same evidence source as receipts. Until then it falls under the command-only rule above.
- The consumer team verifies the fix on its bridge.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 164). Assigned to cl-start, right after #891. Testing is coordinated with the consumer team through the lead.

Root cause (cl-start, 2026-11-05 ~11:15 AM ET): for an agentcloud record, `message_state` answers Processed unconditionally ("journaled at submission"). The bridge checks it before submitting, so it records the request delivered and never runs `agentcloudctl send`.

Reply path folded in at the lead's request (2026-11-05, 12:40 PM ET). During the interim repoint of the consumer team's bridge to its agentterm pane (harness unknown), inbound delivery worked, but the agent's reply blocks for requests 049 and 050 never reached Slack. The prompt still offered fences although no transcript was followed, so the agent posted both by hand with `chat publish`. #852's progress notes say prompts switch to command-only with the target kind, but that did not hold after a runtime repoint.

The reply-154 request itself (the lead, 2026-11-05 06:24 AM ET, P1) was lost in the #884 pane-lock window, in the coordinator's failed/ queue as possibly_submitted. Its full spec was recovered at 12:50 PM ET and is folded in above.
