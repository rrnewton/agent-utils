---
title: 'replication-c6-op-key-marker: every post carries its operation key invisibly, through the provider trait'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T17:02:32.338013293+00:00
updated_at: 2026-10-23T17:02:32.338013293+00:00
---

# Description

[opus 5.5] #454 replication-model C6 and owner Q4. The owner, 2026-10-23: "I hope our backend-agnostic traits have a generic way to inject and read this invisible metadata that works across providers without having to switch on the backend." ChatClient::op_key_carrier(channel) says what each provider carrier gives (dedupes never/within/always, readable); keyed posts hand the key to the native carrier (Slack message metadata, read back with include_all_metadata; a registration bridge nonce, the client-assigned message id on Google Chat; Discord nonce + enforce_nonce); reads surface a carried key in the neutral Message::op_key, which the cache keeps write-once. Part 2, after #448 send-dedupe lands: resolve an uncertain single part by reading upstream, matching on op_key where readable and on text only as the fallback.

# Acceptance Criteria

Per-provider unit tests and a fake that carries and lacks the key; part 2 lands with C5.
