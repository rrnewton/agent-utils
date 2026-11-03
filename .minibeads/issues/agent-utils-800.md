---
title: 'agent-teams-relay-signature-scope: sign envelope kind/topic/supersedes fields'
status: closed
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-03T00:52:06.961097167+00:00
updated_at: 2026-11-03T00:52:06.961097167+00:00
closed_at: 2026-11-03T19:11:04.522858+00:00
---

# Description

[muse watermelon-full-preview] Filed from the #790 review (note 5): the relay signature covers only from/to/id/body_sha256/times, not kind/topic/supersedes/in_reply_to/private/key. Anyone holding the shared bot token could re-post an in-flight message with those fields changed (the ledger's ID dedupe stops re-delivery once the original lands). Extend envelope_material with a versioned scope covering those fields, keeping old envelopes verifiable.
