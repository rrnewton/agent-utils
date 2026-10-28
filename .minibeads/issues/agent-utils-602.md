---
title: 'history-settings-and-limits: per-channel history mode and progress in Settings, a prioritize-now bump, provider rate limits honoured by the crawler, and a smaller cache file'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- cache
created_at: 2026-10-28T01:21:01.017660682+00:00
updated_at: 2026-10-28T03:08:51.618070303+00:00
closed_at: 2026-10-28T03:08:51.618070143+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27 18:11: "Switch our vibe-talk to full-history mode for all of its channels. I assume it is a per channel setting? I think we should expose it under the channel pane in settings. Are our messages on disk compressed? ... A year of messages for a gigabyte sounds fine. We need to have some rate limiting on our crawling because we don't want to get rejected by the upstream Slack or Discord service. Depending on how aggressive they are, maybe we could just respond to any errors with backoff, or maybe we need to set our own rate limit. You should search online to see what the policies are for these services. It would be nice to show progress and ETA in that same channel settings pane." Follow-up to #595 cache-full-history.

Want: (1) a per-channel history mode (recent / full, inheriting the instance default), kept on the server and set in Settings > Channels, with the channel's progress there (oldest reached, complete or walking, pages per hour, ETA); (2) a per-channel prioritize-now bump in the backfill scheduler with an API (POST to bump, GET progress) for a later Search further back button (owner 18:14); (3) the providers' published rate-limit policies researched and cited, and a crawler that paces itself under a conservative per-provider default AND honours every server signal (Retry-After, X-RateLimit headers, exponential backoff on 429 and 5xx with a ceiling), and never trips Discord's invalid-request ban; (4) the cache file's on-disk layout reported, cheap in-database wins measured and the clear ones implemented, and filesystem compression (btrfs zstd) evaluated as a deployment option (owner 18:17).
