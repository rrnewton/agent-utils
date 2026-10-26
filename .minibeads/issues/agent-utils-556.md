---
title: 'chat-delivery-latency: owner chat requests reach the coordinator 5-56 minutes late'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- chat
- agent-queue
created_at: 2026-10-26T21:32:19.829086498+00:00
updated_at: 2026-10-26T21:32:19.829086498+00:00
---

# Description

[opus 5.5] On 2026-10-26 the owner's chat-bridge messages reached the coordinator 36-56 minutes late, and routinely 3-10 minutes late. Nothing on screen said why.

Causes, measured from the coordinator queue's processed records and live timings:

1. Every managed send to an agent holds the agent's lifecycle lock from enqueue through drain and confirmation, so delivery is strictly one at a time; each item's queued_at equals the previous item's confirmed_at. Waiters poll the lock every 10 ms with no order and no priority. The chat bridge delivers its own requests one at a time too, so an owner message competes on equal terms with every agent's agentctl send. One request waited 31 minutes while 15 agent and timer messages went through ahead of it.
2. Each delivery takes 1-2.5 minutes because the misroute check reads the full 400-line scrollback of the target and of every other running agent's pane, serially, before and after typing. The shared terminal server answers a scrollback read of a busy pane in up to 12.5 s (6 ms for an idle one). With 9 running agents that is roughly 18 serial reads per delivery. The same reads issued concurrently finish in about 13.6 s against about 65 s serially.
3. A queue lane created 0755 by another tool (agent-teams' tombstone writer, see agent-teams-cancel-perms) made the bridge's queue lookups fail with 'queue state directory is not private', and two requests stalled for 30 minutes until someone ran chmod by hand.
4. While a request waits, neither chat status nor the agent watcher says who holds the lock or for how long.

# Acceptance Criteria

Bridge requests take priority over ordinary agent-to-agent sends for the lifecycle lock. Peer scrollback reads run concurrently. A non-private lane that only its owner can write is repaired, not fatal; one that others can write is refused with a clear remedy. Waiting is visible: lock holder and waiters are recorded and surfaced by agent status, chat status, and an agent-watcher notice after N minutes.
