---
title: 'relay-never-lossy: the agent-teams relay and the chat bridges must never drop, hold or delay a message on a fragile check; a hash mismatch is delivered with a warning'
status: open
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agent-teams
- agentctl
- chat
- bridge
created_at: 2026-11-27T19:08:38-05:00
updated_at: 2026-11-27T19:08:38-05:00
---

# Description

The agent-teams relay quarantines a relayed message whose body no longer matches its signed hash, and holds it until someone acts. On 2026-11-27 it had quarantined 50 copies of one partner team's messages; Slack had rewritten their bullets (#2160). Every one had also arrived directly, so nothing was lost that time, but a relay-only message would have been. #2160 (landed d8aae6a1) made every observed copy verify, and kept copies of delivered messages without an alarm. A mismatch with no delivered copy still quarantines.

Owner decision, 2026-11-27 7:05 PM ET: "Investigate further -- that is almost certainly our bug. I DO NOT want us just dropping messages because of fragile checks like this. It should be a warning at most and not delay sending the messages. I do NOT want a lossy transport. We NEVER EVER get these kind of gates right. We add a million fragile gates that introduce new failure modes."

So no check in the relay or the chat bridges may lose or delay a message unless it is a real security boundary, and the owner decides those one by one.

# Design

- **Hash mismatch:** deliver the message at once, with a visible warning tag on the delivered text naming the mismatch, and count it in `relay status`. Never quarantine, hold, drop or delay it, and never raise an alarm. #2160's escaping and restore stay, so mismatches become rare. The #2160 grace period and twin settling are then no longer needed for this case.
- **Audit:** every check that can drop, hold or delay a message, in the agent-teams relay and in the chat bridges (`agentctl chat run` and its bridge, `chat-subscription-slack`). That covers quarantine paths, signature and format refusals, size limits, the reply flood guard that holds replies for minutes, dedupe and any other.
  - The result is a table: check, file:line, what it does on failure, and the proposal.
  - Each check that loses or delays becomes warn-and-deliver, unless it is a real security boundary (for example an unknown sender, or a signature that proves impersonation). Those are listed separately, with the reason, for the owner to decide.
- **The quarantined copies:** deliver any whose message was never delivered, or mark each as a duplicate of its delivered twin by message ID, so nothing is lost; report which. Read-only first: the plan for the quarantine state goes to coordb before any change. Dismissing the integrity alarm is part of this, but only after every copy is accounted for.
- **Notice:** changes to the envelope or to persisted fields go to the other teams that run the tool before landing.

# Acceptance Criteria

- A relayed message whose body fails its hash, from an admitted sender, is delivered at once:
  - its delivered text carries a warning tag;
  - `relay status` counts it;
  - no alarm is raised and nothing is quarantined.
  A test proves this for signed and unsigned envelopes, with and without a delivered twin.
- The audit table, checked in under `ai_docs` of the harness repo or in this issue's notes, lists every drop, hold or delay check in the relay and the chat bridges with file:line, failure behaviour and proposal. The security boundaries are listed separately for the owner.
- Each warn-and-deliver change the owner accepts has a test showing the message is delivered, with its warning, on the failing input.
- Every copy in today's quarantine is accounted for: delivered, or matched to its delivered twin by message ID. The report lists each, and the integrity alarm is dismissed only after that.

# Notes

- Follows #2160 (bullet rewrite: escaped by senders, restored by receivers), landed d8aae6a1. P0 by owner decision on 2026-11-27.
- Related: #794 (link markup), #800 (signature v2 downgrade), #2338 (the chat request store no longer refuses silently).
