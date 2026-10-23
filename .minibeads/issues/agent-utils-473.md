---
title: 'replication-c7-server-versions: send the server''s versions to the page and retire read-send order'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T22:49:27.255595376+00:00
updated_at: 2026-10-23T22:49:27.255595376+00:00
---

# Description

[opus 5.5] #454 replication-model C7 (finding F3, race R9). Server half: every served copy carries observed_ms (this server's clock: read start capped by as_of, a live change's read start or arrival, an own post's request time) and every newest page observed_ms plus settled_below_ms (rule 4's proven stretch); deltas and step-backs prove no absence. Page half, after #466 reply-promotes-thread lands: order copies by server version (edit time first) and drop a held copy on absence only when it was observed before the page's read began and created below settled_below_ms, which retires read-send order as the page's clock and makes the interim R9 receipt fix unnecessary.

# Acceptance Criteria

Server fields on every served page and copy, tested; page orders by them with the old order as fallback for older servers; R9 reproduction stays fixed.
