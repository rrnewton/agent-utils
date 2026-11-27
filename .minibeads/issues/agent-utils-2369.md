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
updated_at: 2026-11-27T05:39:34-05:00
---

# Description

Found while investigating #2366 on a consumer team's partner-channel bridge, from its `bridge.log` and raw `plugin.log` windows (2026-11-27; identifiers only).

- The plugin reopens the event stream when the cursor has not moved for 120 minutes.
  - The reopen at 05:35:53Z worked. The new client printed "Resolving FBID", "Resolved user", and then within 1 ms "Using port: 45591", caught up and streamed. The reopen at 07:41:03Z also worked.
  - At 09:46:14.597Z the next reopen ("cursor refresh: the cursor has not moved for 120 minutes; reopening the event stream from sequence 317669") closed the old client.
  - The new client resolved the user at 09:46:19.577Z and exited 20 ms later with `ERROR [Errno 98] Address already in use`, before any "Using port" line.
- Every reconnect then failed the same way, 18-20 ms after "Resolved user" and before "Using port".
  - The provider backed off 1, 2, 4, 8, 16 and 32 s, then reconnected every 60 s: 28 failures, the last at 10:09:26.737Z.
  - Each new subscription logged its "subscribed ... catchup=Sequence(317669)" line, then failed.
- The reconnect at 10:10:29.393Z printed "Using port: 48731" 2 ms after "Resolved user", caught up from 317669 and fetched 1 event, a partner reply posted at 10:02:22Z.
- For 24 minutes the bridge received no stream events. That reply arrived 8 min late, and through #2366 so did an older reply that only that one made visible.
- The bridge's own log says only "subscription backend failed: stream_closed: the stream process closed its output; reconnecting in 60s". The reason, Errno 98, is visible only in the plugin log.
- Bridges on another host show no Errno 98 in about 150 stream starts, and their clients' ports vary from start to start.

# Design

The failing client exits about 20 ms after it resolves the user, before it prints a port, so it fails at the bind of the address it reports with "Using port". The same failure repeats for 24 minutes while the reopen that worked printed its port 1 ms after "Resolved user". So each failing attempt tried an address that something still held. Hypotheses, in order:

1. The client closed at the refresh outlived its close. The plugin stops a stream client by sending SIGTERM to its direct child, then SIGKILL after 3 s (`TAIL_STOP_GRACE`, `stop_child` in `rs/chat-subscription-slack/src/cli.rs`). The child is not started in its own process group, and the group is never signalled. If the client runs its listener in a helper process, that helper keeps the port after the close. A new client that reuses a saved or derived port then fails to bind until the helper exits.
   - Discriminator: the port the 07:41:03Z client printed. If it was 48731, the old client's listener held the port from 09:46Z to about 10:10Z.
2. Another process on the host, for example a second tail for the same identity, held the same address for those minutes.
3. The client picked its port from a small range or from saved state, and repeatedly picked one in use.

Fix plan, once the cause is named:

- Start the stream client as the leader of its own process group. On close, send SIGTERM to the group, SIGKILL it after the grace period, and reap the child. Then no helper outlives a refresh or a dropped subscription.
- Classify an Errno 98 exit as its own failure code (`stream_address_in_use`) and log it in the bridge log, not only the plugin log.
- Let `chat health` report the stream as DEGRADED after 5 consecutive failed reconnects, and DOWN after 15 minutes with no working stream.

# Acceptance Criteria

- The cause is named from the raw plugin log lines (identifiers only), including the port of the client the 09:46Z refresh closed, and from the process tree at a refresh.
- A deterministic test with a fake stream client that starts a child holding a local port: after the plugin closes the stream, the child is gone and a new client can bind the port at once. The test fails on the current code.
- A reconnect loop that fails on Errno 98 is logged in the bridge log with its cause, and shown by `chat health` per the thresholds above, with a test.

# Notes

- Sibling of #2366. Same window, separate cause and fix.
- The bridge's state and logs were read-only for agent-utils lanes. The consumer team extracted them, redacted.
