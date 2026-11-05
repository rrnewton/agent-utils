---
title: 'relay-mirror-rewalk: a pump re-walk must not mirror stale envelopes, and relay trust is per (sender, team)'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-05T03:32:33.361512037+00:00
updated_at: 2026-11-05T03:32:33.361512037+00:00
---

# Description

On 2026-11-04 (03:20-03:21Z), a plain `agent-teams pump` on the lead host re-walked about 528 outbox entries for two teams. They had sat in "accepted" since 10-25..11-03: delivered via node-exec or locally, with no receipt ever recorded. A newly working route answered, so each re-walk mirror-posted the STORED original envelope to the chat relay:
- 162 agent-utils envelopes were pre-#800 v1-signed, so the receiving relay refused them as a "downgrade";
- 185 of the other team's envelopes were unsigned (172) or signed with that team's own key (13), all under the one shared relay sender ID, so they were refused as "key does not match".

The mirror's 60/h rate limit stopped the flood. Delivery was safe: the receiving ledger returns the recorded state for known IDs, so nothing was delivered twice. But the other host filled with quarantines and stranger alarms that cannot be cleared without confirming or banning the shared sender.

# Design

(a) A re-walk must never mirror-post a stored envelope. Mirror only on the first hand-over, or at least re-sign the envelope with v2 under its own team's key.
(b) Stale "accepted" outbox entries need a receipt/reconcile step, so a plain `pump` doesn't re-walk hundreds of them. Alternatively, a plain `pump` acts like `--deferred` unless `--all` is given.
(c) Several teams mirrored from one host share one chat sender, but relay trust is per sender. Confirm keys per (sender, team), so one sender can carry several teams' envelopes, each under its own team key. (The owner asked for the other team's traffic to be mirrored too, so per-(sender, team) keys are preferred over "mirror only your own team".)
(d) Stranger alarms that are known-benign need a dismissal path that neither confirms (which would deliver the quarantined posts) nor bans the sender (which would drop future legitimate mirror posts).

# Acceptance Criteria

- Tests:
  - a re-walk of an accepted entry posts no mirror copy, or posts one re-signed v2 under its team key;
  - plain `pump` leaves stale accepted entries alone (or reconciles them);
  - per-(sender, team) confirmation admits two teams' envelopes from one sender, each under its own key, and refuses a team/key mismatch;
  - a benign stranger alarm can be dismissed without confirming or banning.
- Userguide updated; gate passes.

# Notes

P1 via the lead (2026-11-04), root cause traced read-only by the lead host's helper. Assign to whoever is free after the current P1s.
