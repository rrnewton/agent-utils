---
title: 'cache-backfill: build the declared [message_cache] backfill walk'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- cache
created_at: 2026-10-23T13:16:00.448252765+00:00
updated_at: 2026-10-23T14:12:53.130958881+00:00
closed_at: 2026-10-23T14:12:53.130958741+00:00
---

# Description

[opus 5.5] [message_cache] backfill = "adaptive" is the default and ServerFill::Backfill and ServerFill::Refresher are declared, but nothing calls them, so the cache fills only from reads that happen to be made and the shadow finds almost no read servable (2 of 299 on one Slack deployment). Build a background walk, one provider call at a time and only while the provider is otherwise idle, that lists each channel's newest timeline page and then older pages under the backfill grant until the cache can prove the newest page complete, and report the shadow's servable rate before and after.

# Acceptance Criteria

With backfill adaptive the shadow's servable count for the newest timeline page rises on a fake; backfill off makes no call; the walk yields to other provider use; numbers reported.

# Notes

[opus 5.5] Delivered: [message_cache] backfill = "adaptive" walks each cached channel's newest pages back, only while its provider is idle, up to 20 pages, again every six hours. Measured on the fake: the newest-page shadow rate rose from 0/10 to 10/10 because of the #452 slack-live-ingest relist; the backfill alone did not move that consumer (it adds older history depth). Unverified on a live deployment.
