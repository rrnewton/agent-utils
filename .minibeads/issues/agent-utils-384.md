---
title: 'cache-serve: answer page loads from the message cache at once and refresh in the background'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T22:01:38.206450096+00:00
updated_at: 2026-10-20T01:25:49.578633633+00:00
closed_at: 2026-10-20T01:25:49.578633443+00:00
---

# Description

[muse watermelon] #208 persistent-cache, milestone M9. Behaviour wanted: when an operator has turned the message cache on and serving on, loading or reloading a channel page shows the messages the server already holds on disk at once — within a second, even while the chat provider takes tens of seconds to answer — and the provider is still read in the background, so anything that changed meanwhile (an edited or deleted message) is put right on the page when that read lands. A channel whose held copy is in doubt is never answered from the cache; it waits for the provider as today. With serving off (the default) or no cache table, every read behaves exactly as it does now, and the shadow counters keep comparing what the cache answered, or would have answered, with what the provider answers. Design: ai_docs/transient/2026-10-19-cache-serve.md, including how each requirement #297 cache-coverage-races left for M9 is met or the read is not served. Acceptance: a reload behind a provider that does not answer for 20 seconds shows the held messages within 1 second; an upstream edit and deletion found by the background refresh correct the page; a channel in doubt is not served; measured reload times before and after are reported with the work.
