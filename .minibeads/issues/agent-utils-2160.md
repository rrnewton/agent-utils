---
title: 'agent-teams-relay-slack-bullet-rewrite: the relay quarantines messages whose bullets Slack rewrote'
status: open
priority: 1
issue_type: bug
assignee: cl-bridgeretire
labels:
- agent-teams
created_at: 2026-11-23T08:05:00-05:00
updated_at: 2026-11-27T17:22:21-05:00
---

# Description

The agent-teams Slack relay quarantines messages from a confirmed sender because the body no longer matches its signed hash after undoing Slack link markup. Comparing a quarantined body byte-for-byte with the same message delivered by the direct route shows the only difference: every bullet character U+2022 followed by a space was rewritten by Slack to a hyphen and a space. Three messages were quarantined in one morning, each raising a repeating alarm, though nothing was tampered with.

On 2026-11-27 a relay had quarantined 45 such copies of one partner team's messages, and the alarm repeated on every new one. Comparing each with its direct copy:
- The only difference in all 45 was that every U+2022 had become `-`: 186 bullets, none left.
- 158 of them were line-leading (26 indented), 25 inline after a space, and 3 next to a quote.
- In every case the direct copy had already been delivered with the signed hash, 3-117 s before the relay copy (median 27 s), so nothing was lost.
- A restore of line-leading `- ` alone would recover 37 of the 45.

# Design

- Sender: the relay fence's JSON escapes the bullet glyphs (U+2022, U+2023, U+2043, U+25AA, U+25CF, U+25E6) as `\uXXXX`, like the characters Slack auto-links (#794). The parsed envelope is identical, so any receiver, old or new, gets the signed bytes. There is no new envelope field and no signature change.
- Receiver, for older senders: after the Slack link-markup undo, restore bullets.
  - A `-` followed by a space, a quote or the end of the body may have been a bullet. Every subset of those positions is tried, bullets first, with up to 16 positions and 256 MiB of hashing.
  - Past that, only two bodies are tried: every position a bullet, and only the line-leading ones.
  - A restore is accepted only on an exact body-hash match. The signature covers the hash, so a signed envelope still verifies.
- A body that still fails, when the receiving team's ledger already holds the same message (same ID, same hash), is a provider's damaged duplicate. It is kept as evidence with `twin` (that copy's state and time), counted in `relay status`, and never alarmed.
- Any other mismatch is kept with `awaiting_twin_until`, 10 minutes later, because a relay copy can arrive first. If the same message is delivered by then, the post is settled as a twin. If not, it joins its sender's integrity alarm, which is raised as now.
- The alarm is filed under the team the message was addressed to, not the first team whose relay channels share the route.
- Posts and alarms kept before this change are left exactly as they are.

# Acceptance Criteria

- A test with a signed body containing bullets, rewritten the way Slack rewrites them, verifies and is delivered; a body changed any other way is still quarantined.
- A new sender's fence carries no raw bullet glyph and survives the rewrite unchanged. An older sender's post with every observed shape rewritten (line-leading, indented, inline, quoted, mixed with genuine hyphens) is restored to the exact signed body and delivered.
- A damaged relay copy of a message already delivered is kept without an alarm and counted in `relay status`. One that arrives before its message is delivered never alarms once that copy arrives.
- A damaged copy with no delivered copy alarms after the grace period, under the team it was addressed to.

# Notes

Found by comparing a quarantined message with its direct-route copy; filed by the coordinator.

Raised to P1 on 2026-11-27, when the open integrity alarm had grown to 45 copies.

Out of scope, but seen in the same state directory: every atomic rewrite keeps a timestamped backup beside its file. There were 1,280 for the quarantine file, 1,075 for the alarms file, and over 20,000 in one team's ledger. See #599, #767 and #856.
