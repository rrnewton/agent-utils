---
title: 'agent-teams-relay-large: size-aware relay posts and a JSON-only mode per provider'
status: open
priority: 1
assignee: mu-mirror@devbig014
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.764058478+00:00
updated_at: 2026-11-02T16:26:35.764058478+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). per-provider size limits (Slack ~11k split/40k cap; Discord bots 2000 chars, embeds 4096/6000, attachments ~10MB): small = one post; large = header + body excerpt + envelope as attached file <id>.agent-teams.json (signed, authoritative); no-attachment providers = numbered signed part-posts reassembled, never partial. Owner OK with JSON-only posts (no human-readable copy) as a mode; a viewer utility (e.g. agent-teams relay show/tail) renders them. Next relay slice after #722 fix + backpressure.

# Notes

[opus 5.5] 2026-11-02, mirror rollout: a mirror copy of a ~5.6 KB message was published as TWO top-level relay posts (visible text 2,479 chars with no fence; the fenced envelope alone, 3,177 chars), because the posting helper goes through the bridge's Slack outbound path, which splits long messages. The visible half was quarantined as unparseable and raised a STRANGER alarm for the (confirmed) shared bot; the direct delivery was unaffected. Now the top item for the lane, ahead of #769. Asks: (1) a relay post is never split: put the fenced envelope first and keep each post under the outbound split limit, or truncate the visible copy, or upload large envelopes as a file; (2) an unparseable post from a CONFIRMED sender alarms as "unparseable relay post", not as a stranger; (3) a CLI to dismiss a quarantined post. Until it lands, expect one alarm per long mirrored message; ignore (do not confirm or ban).

[opus 5.5] Follow-up (2026-11-02, after the 03ed5c8f rollout): `relay quarantine dismiss` does not close a LEGACY alarm written by an older build without a "kind" field (drop_quarantined only closes kind=unparseable); both hosts needed `relay confirm` of the shared bot to clear it. Dismiss should also close a kind-less alarm that names the dismissed post.
