---
title: 'agentctl-muse-store-stale-lock-pid-reuse: reject stale session locks after pid reuse in the Muse store lookup'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- muse
created_at: 2026-11-20T05:05:00-05:00
updated_at: 2026-11-20T05:05:00-05:00
---

# Description

[watermelon-full-preview] Follow-up from the second review of
#1934 agentctl-restore-muse-sessions-and-timeout (SAFE, with this
residual): the Muse store lookup by pid can match a stale lock
after pid reuse (1063 locks name 750 pids) in the roughly 3–13 s
before Muse writes its own lock for the new session. A reused pid
can therefore attribute a dead lane's session to a live one in that
window.

# Design

- Require the lock's modification time to be at or after the pinned
  process's start time, or trust only Muse's own runtime JSON
  report (`runtime/muse/sessions/*.json` by verified pid) and drop
  the lock-content match.
- Add the missing tests: reused-pid rejection (the test build
  currently stubs the identity check true), Muse's report preferred
  over a held peer lock, the store-root filter on held paths, and a
  candidate-proof/pin identity mismatch.

# Acceptance Criteria

- A stale lock naming a reused pid is never attributed, even in the
  window before the runtime writes its own lock.
- The new tests fail on the #1934 code and pass with the fix.
- Focused revive suites stay green.

# Notes

[watermelon-full-preview] Filed by mu-restorefix at coordb's
request; does not block the #1934 landing. Related: #1934
agentctl-restore-muse-sessions-and-timeout.
