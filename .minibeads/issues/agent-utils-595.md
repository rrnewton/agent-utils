---
title: 'cache-full-history: the server gathers each channel''s whole history into its cache, and re-checks the recent part in tiers'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- cache
created_at: 2026-10-27T20:55:54.699605677+00:00
updated_at: 2026-10-27T22:16:39.477314384+00:00
closed_at: 2026-10-27T22:16:39.477314204+00:00
---

# Description

[opus 5.5] Owner, 2026-10-27 13:44: "I think we should focus on rock-solid search of our own server first ... if we stand up a new server and Slack or Discord has been running for months... what happens? ... we could have another policy where, at some steady background rate, we keep loading history further and further back until we have the whole thing in our server. Once we have the whole history, our search becomes complete." Refinements 13:48: the backfill mode is a setup-time choice, recent (default, 14 days) or full; tiered revalidation (last 24 h hourly, the 14-day window daily, and after any push-stream gap), intervals as config knobs, API reads per hour measured before and after on the loopback fake; document what the re-walk is for.

Want: (1) backfill = "full" resumes older and older across walks and restarts from a persistent per-channel frontier; (2) no age cap in full mode (max_megabytes stays the bound), a steady per-provider pace within documented rate limits, idle-only, yielding to pages, the poller and voice; (3) every cached channel, plus thread replies; (4) a per-channel complete-history marker surfaced in the cache status and diagnostics with progress (oldest date reached, pages per hour, ETA); (5) a storage estimate from the live store, with a cap knob, and a cache stats subcommand so measuring needs no copy.
