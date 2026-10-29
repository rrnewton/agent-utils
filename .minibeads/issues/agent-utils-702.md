---
title: 'send-hard-timeout: agentctl send and drain give up after a configurable wall-clock deadline (default 10 min)'
status: open
priority: 0
issue_type: bug
created_at: 2026-10-29T21:51:37.577251764+00:00
updated_at: 2026-10-29T21:51:37.577251764+00:00
---

# Description

Owner, 2026-10-29 17:47 ET: "a message that doesn't send for 10 min should definitely abort...
maybe 5 min. It should be part of the agentctl config in a given project, but default to 10."

Context: that day 13 sends from an old build sat 11 to 23 hours holding an adopted
coordinator's agent lock, blocking its chat bridge and their own callers. A managed send takes
the agent lifecycle lock with no bound and holds it through every readiness wait.
`DrainOptions.deadline` already bounds queue and target lock waits and readiness waits, but
nothing set it by default.

## Wanted

- A hard wall-clock deadline for `agentctl send` and `agentctl drain`. It is counted from the
  start and covers every wait: the agent lifecycle lock, the queue and target locks, and
  readiness. A prompt already being typed is finished; that is bounded by `--working-timeout`.
- Default 600 s. Configurable per registry (project), read tolerantly so that older binaries
  never break. Overridable per call with `--deadline SECONDS`.
- On expiry: stop waiting, release every lock (the process exits), leave the prompt queued as
  pending (safe to retry), and exit 75 with a message that names the deadline. If the deadline
  expires while still waiting for the agent lock, before anything was queued, say that nothing
  was queued.
- Tests for the deadline, the lock release, and the configuration precedence.

## Follow-up (separate issue)

Never hold a lock while waiting for the pane to become idle: enqueue under the lock, release
it, wait for readiness without any lock, then lock again and type.
