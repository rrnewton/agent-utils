---
title: 'vibe-talk-freeze-day-counters-persist: page freeze day counters survive a server restart'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- telemetry
created_at: 2026-11-22T14:25:00-05:00
updated_at: 2026-11-22T14:25:00-05:00
---

# Description

The page-freeze telemetry keeps its per-day counters in memory only, so every server restart resets them (one day showed 214 day reports before a restart and 4 after), and per-day freeze rates read lower than they are on any day with a deploy.

# Design

Persist the per-day counters in the existing store (or rebuild them at startup from the persisted freeze reports for the current day), so health output and the day report stay correct across restarts. Keep the in-memory path as a cache.

# Acceptance Criteria

- A test restarts the server mid-day and the day counters continue from their earlier values.

# Notes

Noticed by the liaison; filed by the coordinator. Not urgent.
