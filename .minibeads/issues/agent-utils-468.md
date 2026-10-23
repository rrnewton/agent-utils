---
title: 'send-dedupe: settle a lost answer from the stream echo and on reload, persist post keys, look upstream for an uncertain part'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- send
depends_on:
  agent-utils-448: related
created_at: 2026-10-23T17:54:47.538598342+00:00
updated_at: 2026-10-23T17:54:47.538598342+00:00
---

# Description

[opus 5.5] Follow-up to #448 send-dedupe, from the replication model (ai_docs/transient/2026-10-23-replication-model.md §7).
- C2: carry the idempotency key on the self-posted stream event (`client_key`), and have the page settle an entry by it exactly as from a 200, so a lost 200 never leaves a real message beside an unconfirmed row offering Retry.
- C3: after loading the outbox, ask POST .../reply/outcome once for every unconfirmed keyed entry; 200/207 settles it, post_unknown leaves it unconfirmed.
- C5: persist post keys in the store (24 h, bounded), so a restart answers a repeat of a landed post instead of post_unknown.
- C6 (owner 2026-10-23: yes): after an uncertain send on a provider without upstream keys, look upstream before offering Retry. Owner, answer 4: use an invisible operation-key marker where the provider allows it — Slack message metadata ({event_type, event_payload}, invisible in the client, readable with metadata included), Google Chat client-assigned message ids, Discord nonce with enforce_nonce — and match on it rather than on text.
- Owner, answer 3: Edit of a send that may have posted makes a new message with a new key (append-only) and warns that the old one may already be in the channel; Edit is never refused.
