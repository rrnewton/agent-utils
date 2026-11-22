---
title: 'vibe-talk-cache-missing-owner-messages: the local message cache lacks owner messages that a coordinator saw'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-11-22T15:30:00-05:00
updated_at: 2026-11-22T15:30:00-05:00
---

# Description

A prompt search over the local message cache (the prompts reader, cache-only) missed 15 of the owner's messages in one channel on four days (seven within one hour on one morning), although a coordinator's transcript shows they were delivered. They are in neither the cache database nor its write-ahead log. One whole channel the owner uses is not in the cache at all.

# Design

Find why those messages never reached the cache (ingest gap during a restart, a cursor that skipped, a channel not configured for caching, a rescan that never covered them) and backfill. Add a consistency check that compares the cache against a reliable listing for a recent window and reports gaps, so the prompts reader and any search built on the cache (#2106) can be trusted as "every owner message".

# Acceptance Criteria

- The cause is named; the missing messages are backfilled; a check reports any future gap; a test covers the failure mode.

# Notes

Found by the Buck/OSS prompt search; filed by the coordinator.
