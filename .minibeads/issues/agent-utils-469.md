---
title: 'replication-model-tests: model-based tests against random interleavings'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T17:02:40.555155563+00:00
updated_at: 2026-10-23T17:02:40.555155563+00:00
---

# Description

[opus 5.5] #454 replication-model section 8. A seeded xorshift generator, a reference model of the provider, and random interleavings of provider events, live-feed deliveries (out of order) and reads that start and land out of order, some answered from a memory that says how old it is; properties checked after every step (no resurrection, no reversion, nothing invented) and convergence at quiescence. Server cache first; the page driver (voice_page.test.mjs) follows after #448 send-dedupe lands. The first run found a real gap: a pushed creation delivered after reads saw an edit replaced the edited text (merge rule 2 had no edit time to order it by); a pushed creation is now older than any held copy with an edit time.

# Acceptance Criteria

The cache harness runs in the gate; the page harness covers properties 2, 3, 4, 7.
