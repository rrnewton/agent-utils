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
updated_at: 2026-11-27T19:21:12-05:00
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
- Quarantine reconciliation, 2026-11-27, read-only before any change. The relay quarantine held 49 posts, all body-hash mismatches from one confirmed sender.
  - Every one has a delivered twin: the receiving team's ledger entry with the same message ID and the same signed body hash, in state `accepted`.
  - None was relay-only, so nothing was lost.
  - The #2160 bullet restore reproduces the signed body for 48 of the 49.
  - The posts are dropped and the integrity alarm dismissed after this list is recorded (owner decision). IDs are shown by their 16-hex suffix:
  - `1579fa34fd9fd191` (partner team): quarantined 2026-11-23T10:56:49Z; delivered copy `accepted` at 2026-11-23T10:55:33Z
  - `679d5ca5b7476fac` (partner team): quarantined 2026-11-23T11:44:20Z; delivered copy `accepted` at 2026-11-23T11:43:52Z
  - `6b668193c4f77324` (partner team): quarantined 2026-11-23T12:45:29Z; delivered copy `accepted` at 2026-11-23T12:45:01Z
  - `87fcc20e00ff6660` (own team): quarantined 2026-11-23T13:12:20Z; delivered copy `accepted` at 2026-11-23T13:12:06Z
  - `9a3fd9b558de7e99` (partner team): quarantined 2026-11-23T13:31:53Z; delivered copy `accepted` at 2026-11-23T13:31:28Z
  - `5e0493b3c75f4ce0` (partner team): quarantined 2026-11-25T03:20:38Z; delivered copy `accepted` at 2026-11-25T03:20:35Z
  - `c82696b07635dd01` (partner team): quarantined 2026-11-25T13:17:54Z; delivered copy `accepted` at 2026-11-25T13:17:28Z
  - `63fad355a42b0494` (partner team): quarantined 2026-11-25T16:01:18Z; delivered copy `accepted` at 2026-11-25T16:00:41Z
  - `7785c9c8b5c48e43` (partner team): quarantined 2026-11-27T10:15:53Z; delivered copy `accepted` at 2026-11-27T10:15:29Z
  - `f72e05d7a2211311` (partner team): quarantined 2026-11-27T11:07:12Z; delivered copy `accepted` at 2026-11-27T11:06:23Z
  - `da6f931e064efc8e` (partner team): quarantined 2026-11-27T11:37:27Z; delivered copy `accepted` at 2026-11-27T11:37:24Z
  - `e2171242be6b054a` (partner team): quarantined 2026-11-27T11:44:46Z; delivered copy `accepted` at 2026-11-27T11:43:58Z
  - `9f882321472e1175` (partner team): quarantined 2026-11-27T11:52:30Z; delivered copy `accepted` at 2026-11-27T11:52:22Z
  - `64acbf7f4385c4c3` (partner team): quarantined 2026-11-27T12:00:25Z; delivered copy `accepted` at 2026-11-27T12:00:12Z
  - `2315d3f1adf56d80` (partner team): quarantined 2026-11-27T12:01:26Z; delivered copy `accepted` at 2026-11-27T12:00:50Z
  - `7e5b22f255a66037` (partner team): quarantined 2026-11-27T12:16:25Z; delivered copy `accepted` at 2026-11-27T12:15:56Z
  - `d51325be81a9b16a` (partner team): quarantined 2026-11-27T12:21:24Z; delivered copy `accepted` at 2026-11-27T12:20:50Z
  - `8120b9a140369325` (partner team): quarantined 2026-11-27T12:38:12Z; delivered copy `accepted` at 2026-11-27T12:37:28Z
  - `f453197e16f20ae5` (partner team): quarantined 2026-11-27T12:40:21Z; delivered copy `accepted` at 2026-11-27T12:40:09Z
  - `7914868e3b85b07f` (partner team): quarantined 2026-11-27T13:15:08Z; delivered copy `accepted` at 2026-11-27T13:14:39Z
  - `3fe93de9186c9449` (partner team): quarantined 2026-11-27T13:46:58Z; delivered copy `accepted` at 2026-11-27T13:46:04Z
  - `299a7f6b49349970` (partner team): quarantined 2026-11-27T14:06:53Z; delivered copy `accepted` at 2026-11-27T14:06:22Z
  - `1de4f285ca2bd5bb` (partner team): quarantined 2026-11-27T14:10:49Z; delivered copy `accepted` at 2026-11-27T14:10:41Z
  - `8bb70d72765c0ba1` (partner team): quarantined 2026-11-27T14:14:35Z; delivered copy `accepted` at 2026-11-27T14:14:06Z
  - `6cf5b949ead23db2` (partner team): quarantined 2026-11-27T15:24:17Z; delivered copy `accepted` at 2026-11-27T15:23:35Z
  - `df6f5716f2d4114e` (partner team): quarantined 2026-11-27T16:08:59Z; delivered copy `accepted` at 2026-11-27T16:08:39Z
  - `6f9b08b38c0bd23d` (partner team): quarantined 2026-11-27T16:14:44Z; delivered copy `accepted` at 2026-11-27T16:14:28Z
  - `b78919c83a415600` (partner team): quarantined 2026-11-27T16:42:32Z; delivered copy `accepted` at 2026-11-27T16:42:06Z
  - `7dece75a2fdd003f` (partner team): quarantined 2026-11-27T17:10:02Z; delivered copy `accepted` at 2026-11-27T17:08:40Z
  - `e8d0e9c5ca00917f` (partner team): quarantined 2026-11-27T18:21:03Z; delivered copy `accepted` at 2026-11-27T18:20:39Z
  - `573a8620754084a7` (partner team): quarantined 2026-11-27T19:51:04Z; delivered copy `accepted` at 2026-11-27T19:50:26Z
  - `9c425856d8ebf3be` (partner team): quarantined 2026-11-27T20:00:07Z; delivered copy `accepted` at 2026-11-27T19:59:33Z
  - `e97bfd70247e4c8e` (partner team): quarantined 2026-11-27T20:07:39Z; delivered copy `accepted` at 2026-11-27T20:07:34Z
  - `20a6590a64d9caea` (partner team): quarantined 2026-11-27T20:10:30Z; delivered copy `accepted` at 2026-11-27T20:10:09Z
  - `5d45dd9375c825b0` (partner team): quarantined 2026-11-27T20:13:15Z; delivered copy `accepted` at 2026-11-27T20:12:49Z
  - `bd44b064040e8125` (partner team): quarantined 2026-11-27T20:13:15Z; delivered copy `accepted` at 2026-11-27T20:12:56Z
  - `b1d829118ebece7b` (partner team): quarantined 2026-11-27T20:13:18Z; delivered copy `accepted` at 2026-11-27T20:13:00Z
  - `e9943bbbea6c9871` (partner team): quarantined 2026-11-27T20:13:18Z; delivered copy `accepted` at 2026-11-27T20:13:04Z
  - `5e8c117881bdd77c` (partner team): quarantined 2026-11-27T20:13:18Z; delivered copy `accepted` at 2026-11-27T20:13:10Z
  - `4881a53eb1ddd38a` (partner team): quarantined 2026-11-27T20:52:56Z; delivered copy `accepted` at 2026-11-27T20:52:23Z
  - `52b5f0827625a572` (partner team): quarantined 2026-11-27T21:04:06Z; delivered copy `accepted` at 2026-11-27T21:02:09Z
  - `1ba59b1de41cf321` (partner team): quarantined 2026-11-27T21:38:08Z; delivered copy `accepted` at 2026-11-27T21:37:47Z
  - `a06c48701a24f3e2` (partner team): quarantined 2026-11-27T21:38:08Z; delivered copy `accepted` at 2026-11-27T21:37:51Z
  - `b21c51574cfbbd08` (partner team): quarantined 2026-11-27T21:47:38Z; delivered copy `accepted` at 2026-11-27T21:46:55Z
  - `d12e86e614eda1dc` (partner team): quarantined 2026-11-27T22:00:34Z; delivered copy `accepted` at 2026-11-27T21:59:18Z
  - `ac370137f8abf2c2` (partner team): quarantined 2026-11-27T22:20:13Z; delivered copy `accepted` at 2026-11-27T22:19:43Z
  - `664f5f90b6053631` (partner team): quarantined 2026-11-27T22:26:01Z; delivered copy `accepted` at 2026-11-27T22:25:24Z
  - `16f92d87abb43a0b` (partner team): quarantined 2026-11-27T23:45:12Z; delivered copy `accepted` at 2026-11-27T23:44:39Z; past the 16-position restore cap
  - `64c698a73c172f6d` (partner team): quarantined 2026-11-28T00:01:09Z; delivered copy `accepted` at 2026-11-28T00:00:24Z
