---
title: 'cache-backfill: build the declared [message_cache] backfill walk'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- cache
created_at: 2026-10-23T13:16:00.448252765+00:00
updated_at: 2026-10-23T13:16:00.448252765+00:00
---

# Description

[muse watermelon] [message_cache] backfill = "adaptive" is the default and ServerFill::Backfill and ServerFill::Refresher are declared, but nothing calls them, so the cache fills only from reads that happen to be made and the shadow finds almost no read servable (2 of 299 on one Slack deployment). Build a background walk, one provider call at a time and only while the provider is otherwise idle, that lists each channel's newest timeline page and then older pages under the backfill grant until the cache can prove the newest page complete, and report the shadow's servable rate before and after.

# Acceptance Criteria

With backfill adaptive the shadow's servable count for the newest timeline page rises on a fake; backfill off makes no call; the walk yields to other provider use; numbers reported.
