---
title: 'relay-signature-v1-floor: reject sig-only envelopes once every host signs v2'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-03T18:54:14.118160019+00:00
updated_at: 2026-11-03T18:54:14.118160019+00:00
---

# Description

[muse watermelon-full-preview] Follow-up to #800 agent-teams-relay-signature-scope: #800 adds envelope signature v2 (sig2 covering kind/topic/supersedes/in_reply_to/private/key) alongside v1, plus a sticky per-peer upgrade — after a trusted key first verifies a v2 signature, sig-only envelopes from that sender are refused as downgrades. During rollout a sender never seen with v2 still passes v1, so the downgrade window remains open fleet-wide for never-upgraded peers.

When coordb confirms every relay host runs a build that signs v2, flip the floor: reject sig-only envelopes from any sender whose key is trusted (the same downgrade quarantine as the sticky path), regardless of whether that key has been seen with v2. Do not guess a date in code; land on the fleet confirmation.

Recovery from a downgrade (both today, per-peer, and after this floor flips): if a peer's build rolls back to v1 after it once sent v2, its sig-only envelopes are downgrade-quarantined. The operator clears that key's sticky record with `agent-teams relay reset-sig-v2 TEAM@HOST` (or `relay untrust-key TEAM@HOST` followed by `relay trust-key TEAM@HOST`); sig-only envelopes are then admitted again until the peer next verifies a v2 signature. The downgrade alarm text names this recovery. Note the matching is per team@host while the record sits on that team@host's one trusted key, so the reset is effectively per pubkey.

# Acceptance Criteria

- A sig-only envelope from a trusted key is refused (downgrade-quarantined) even when that key has never verified a v2 signature.
- Unsigned and unknown-key handling is unchanged.
- Tests cover the flipped floor; the sticky tests from #800 keep passing.
