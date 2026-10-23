---
title: 'send-refusal-reason: say why the chat service refused a send, and never call a refusal unconfirmed'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
created_at: 2026-10-23T01:16:47.176098728+00:00
updated_at: 2026-10-23T01:36:49.362352666+00:00
closed_at: 2026-10-23T01:36:49.362352666+00:00
---

# Description

[opus 5.5] The owner, replying in threads on a Google Chat bridge: "the message won't send after multiple retries. But it seems to fail less when I just send a message to the main channel." Four retries to one thread were each refused by the bridge with HTTP 400 within milliseconds; vibe-talk answered 207 with posted 0, uncertain false, retryable false, and logged no reason. The page treated every 207 as 'Delivery unconfirmed. Retrying cannot post it twice.', which invites exactly the retries that cannot succeed.

Wanted: log a provider's 4xx refusal of a send at WARN (status, provider, channel, thread or main, the provider's reason, content length; never the content); carry a plain-words refusal in the 207 body; the page shows it on the failed outbox card ('Google Chat refused this message: it contains @word, which would notify someone') as Not sent, not unconfirmed, and never retries a 4xx on its own.

# Acceptance Criteria

Unit tests for the refusal wording and logging path; a page test that a 207 refusal with nothing posted draws Not sent with the reason and arms no automatic retry.
