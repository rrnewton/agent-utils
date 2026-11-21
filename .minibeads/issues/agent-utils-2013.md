---
title: 'agentctl-muse-lock-host-attribution: require the session lock host line to match this host'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- muse
created_at: 2026-11-21T05:30:00-05:00
updated_at: 2026-11-21T05:30:00-05:00
---

# Description

[watermelon-full-preview] Muse session-lock attribution ignores
the lock's `host=` line. dotsync copies Muse session directories
between hosts, so a lock written on the other host can name a pid
that happens to exist locally, and a lane can be attributed another
host's session in both the revive and restore paths.

# Design

- Lock attribution (held locks and the pid-keyed store lookup)
  requires the lock's `host=` line to name this host (full name or
  short label); a missing or different host line attributes
  nothing (fail closed).
- Tests use a fixture lock with a foreign `host=`.

# Acceptance Criteria

- A copied lock naming a local pid but a foreign host attributes
  nothing at start, adopt, promote, or store lookup.
- Locks from this host keep attributing as before.
- Focused revive suites stay green.

# Notes

[watermelon-full-preview] Lane mu-restorefix/2013, follow-up from
the #1934 delta review, coordinated by coordb. Related: #1934
agentctl-restore-muse-sessions-and-timeout, #1945
agentctl-muse-store-stale-lock-pid-reuse.
