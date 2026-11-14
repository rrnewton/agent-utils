---
title: 'chat-replies-captured-not-sent: breaker-held replies wait for an unrelated loop pass after the cooldown'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T18:40:00.000000000+00:00
updated_at: 2026-11-14T21:20:00.000000000+00:00
---

# Description

Five liaison chat replies captured 2026-11-14 18:28-18:32Z stayed
captured-not-sent while other replies from the same batch went out. Each
unsent reply record was phase "pending" with send_attempts 0. The replies
eventually went out at 18:36:02-18:36:08Z under their original send
request IDs, with exactly one provider post each.

# Design

- Cause: the reply post-rate breaker is per (channel, thread) — 8 reply
  reservations per 60 s, then a 300 s cooldown. All affected requests
  share one thread, so a burst released after a long delivery hold
  tripped the breaker at 18:28:51Z (held until 18:33:51Z). A breaker
  refusal happens before any provider IO, so held replies keep phase
  "pending" and send_attempts 0.
- Gap: nothing retries a held reply when the cooldown expires. The
  reply sender drops a key whose send fails, and the owner loop retries
  only on its next pass, whose cadence is unbounded. The five replies
  above went out 2 min 11 s after the cooldown expired, on the next
  loop pass that happened to include them.
- Fix: the reply sender remembers a key whose send the breaker held,
  together with the breaker's release time for that request's thread,
  and retries it when that time passes (or sooner if the breaker
  record is repaired or deleted). A restart schedules the same retry
  for pending replies whose thread is held. Retries keep the original
  send request ID, so a held reply is never posted twice.

# Acceptance Criteria

- A regression test with no wall-clock budgets: a reply the breaker
  holds reports the exact release time; a retry before that time makes
  no provider call; after the breaker record ages past the cooldown, a
  retry sends the reply under its original request ID.
- Held replies go out promptly after the cooldown without waiting for
  an unrelated owner-loop pass or a new reply announcement.
- Existing breaker behaviour is unchanged: the per-thread budget, the
  300 s cooldown, per-request reply order, and operator reset by
  deleting the breaker record.

# Notes

Live incident 2026-11-14: trip at 18:28:51Z on the shared thread,
cooldown to 18:33:51Z, sends at 18:36:02-18:36:08Z. Verified from the
bridge state, the bridge log, and the outbound helper log; no manual
release was needed and no duplicate was posted.

Follow-up (to be numbered by the coordinator, not fixed in this lane):
`test_launch_bridge_killed_first_is_an_error_and_stops_coordinator`
in the Python launch tests waits on a 60 s wall-clock budget for the
bridge process to appear, so it can fail under load although the
bridge is healthy; it passes standalone. Replace that budget with a
condition-based wait.

## Resolution

Landed 404a206d (gated 72980192, gate pass 2026-11-14T21:09:55Z): the
reply sender schedules breaker-held keys for their release time,
including at startup, with two regression tests that use no wall
clock. Live on the bridge since the services turnover to 4cbc07e4
(2026-11-14 4:17 PM ET, chat health OK). The launch-test follow-up is
tracked as #1551 agentctl-chat-launch-wallclock-tests.
