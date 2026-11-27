---
title: 'slack-stream-reconnect-address-in-use: after a cursor refresh, 28 stream reconnects in 23 min failed with Errno 98 (address already in use), and the bridge received nothing meanwhile'
status: open
priority: 1
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- slack
- bridge
created_at: 2026-11-27T05:31:21-05:00
updated_at: 2026-11-27T05:31:21-05:00
---

# Description

Found while investigating #2366 on a consumer team's partner-channel bridge, from its `bridge.log` and a pattern summary of its `plugin.log` (2026-11-27, 04:30Z to 10:20Z; identifiers only).

- The plugin reopens the event stream when the cursor has not moved for 120 minutes. It did so at 05:35:53Z and at 07:41:03Z, and both reopens worked: each client printed its local port, caught up and streamed.
- At 09:46:19Z, about when the next reopen was due, the stream process closed its output. From then on every new stream client exited with `ERROR [Errno 98] Address already in use`, before it printed a local port.
- The provider reconnected with back-off (1, 2, 4, 8, 16, 32 s, then every 60 s): 28 failed reconnects, the last at 10:09:26Z.
- The reconnect at 10:10:29Z worked, on a new port.
- For those 24 minutes the bridge received no stream events. A partner member's reply sent at 10:02:22Z arrived 8 min late, and through #2366 so did an older reply that only that one made visible.
- The bridge's own log says only "subscription backend failed: stream_closed: the stream process closed its output; reconnecting in 60s". The reason, Errno 98, is visible only in the plugin log.
- Bridges on another host show no Errno 98 in about 150 stream starts. Their clients' local ports vary from start to start.

# Design

Hypotheses, to be settled with the raw plugin log windows 09:45:00Z to 09:47:45Z, 10:09:15Z to 10:11:10Z and, for comparison, the working reopen at 05:35:40Z to 05:36:20Z:

1. A leftover process holds the address. The plugin closes a stream (at a cursor refresh or when a subscription drops) by sending SIGTERM to its direct child, then SIGKILL after 3 s (`TAIL_STOP_GRACE`, `stop_child` in `rs/chat-subscription-slack/src/cli.rs`). The child is not started in its own process group, and the group is never signalled. If the stream client starts a helper process that binds a fixed local address, the helper outlives the close. Every new client then fails to bind until the helper exits (here about 23 min).
2. Another process on the host, for example a second tail for the same identity, held the same fixed address for those minutes.
3. The client chose its port from a small range or from saved state, and repeatedly picked one in use.

Fix plan, once the cause is named:

- Start the stream client as the leader of its own process group. On close, send SIGTERM to the group, SIGKILL it after the grace period, and reap the child. Then no helper outlives a refresh or a dropped subscription.
- Classify an Errno 98 exit as its own failure code (`stream_address_in_use`) and log it in the bridge log, not only the plugin log.
- Let `chat health` report the stream as DEGRADED after 5 consecutive failed reconnects, and DOWN after 15 minutes with no working stream.

# Acceptance Criteria

- The cause is named from the raw plugin log lines (identifiers only).
- A deterministic test with a fake stream client that starts a child holding a local port: after the plugin closes the stream, the child is gone and a new client can bind the port at once. The test fails on the current code.
- A reconnect loop that fails on Errno 98 is logged in the bridge log with its cause, and shown by `chat health` per the thresholds above, with a test.

# Notes

- Sibling of #2366. Same window, separate cause and fix.
- The bridge's state and logs were read-only for agent-utils lanes. The consumer team extracted them, redacted.
