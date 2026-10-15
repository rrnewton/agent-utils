---
title: 'timeline-single-flight: share one provider read among identical timeline requests'
status: open
priority: 3
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.632164209+00:00
updated_at: 2026-10-15T15:52:38.632164209+00:00
---

# Description

The 2026-10-13 #285 quick-wins-safe landing ported the read-reason tags and 'keep the newer copy' from the timeline single-flight work, but not the single flight itself: share one provider read among identical concurrent timeline requests and window reads. It is preserved on GitHub as archived/vt/timeline-single-flight (14 commits). Main has single flights only for catch-up and the voice pre-read. Re-measure whether identical concurrent timeline reads still reach the provider (two tabs, a poll racing a reload), and port the single flight if they do, with the call counts as the test.
