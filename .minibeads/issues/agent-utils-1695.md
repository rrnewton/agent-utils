---
title: 'bridge-directory-cache-reads: sender, human and member lookups from a cache, not paged Slack reads'
status: open
priority: 2
issue_type: task
labels:
- slack
- vibe-talk
created_at: 2026-11-16T02:00:00.000000000+00:00
updated_at: 2026-11-16T02:00:00.000000000+00:00
---

# Description

The Slack bridge resolves senders, checks that an author is human, and enumerates channel members with fresh Slack reads every time: a user describe call per resolution or rejection check, and a paged member enumeration (20 a page, complete or refused) per grant. The embedded operator guidance also tells a person to run the same describe command by hand. vibe-talk already caches author names and group members for the same workspace, so these reads duplicate cached directory data and burst pages of calls for a large channel.

# Design

Answer directory questions from vibe-talk's directory and name cache (or a bridge-local cache with a stated time-to-live fed by it): sender resolution, the human-author check before a rejection reaction, and member enumeration for grants. A cache miss may do one upstream read and then cache it; a partial enumeration is still refused rather than returned as complete.

# Acceptance Criteria

- Resolving a sender or checking an author that the cache already knows makes zero upstream user or member calls.
- A member enumeration for a grant is served from the cache when fresh, and is refused when the cached list is known incomplete, never returned partial.
- The operator guidance names the cache-backed lookup instead of a direct Slack describe command.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
