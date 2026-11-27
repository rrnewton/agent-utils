---
title: 'slack-old-thread-reply-late: a reply in a thread older than the rescan window reaches the agent only when the stream later carries another reply there (5 h 20 min late)'
status: open
priority: 1
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- slack
- bridge
created_at: 2026-11-27T05:20:51-05:00
updated_at: 2026-11-27T05:31:21-05:00
---

# Description

A consumer team runs a chat bridge from one of its lanes to a channel it shares with a partner team.

On 2026-11-27 a partner member's question (bridge message 1528) was posted at 04:50:24Z. It was a reply in a thread whose root was 38.3 h old, and it came one minute after the lane's own reply in that thread, which the bridge's app posted at 04:49:05Z. The question reached the agent at 10:11:04Z, 5 h 20 min 40 s late, through the Slack plugin's periodic rescan. The lane had read the channel at 07:40Z and reported "no reply yet", so the team acted on the partner's review without the open question.

Evidence comes from the bridge's `bridge.log`, `plugin.log` (a pattern summary plus every rescan pass that made a thread listing) and `slack-rescan-ledger.json` for 04:30Z to 10:20Z, extracted read-only by the consumer team. Identifiers only, no message text.

(a) The live stream never delivered 1528. This part is PARTIAL: where the message was lost is known, why is not yet.

- The owner's event topic carried nothing for the channel from 04:30Z to 10:10Z.
- The two cursor refreshes in the window (05:35:53Z and 07:41:03Z) each caught up from a sequence before 1528 and fetched 0 events. So the event service never published 1528 to the topic; the plugin did not drop it.
- Earlier and later replies in the same thread from the same sender were published: 1526 at 01:30:37Z, and 1527 at 10:02:22Z.
- Why 1528 was not published is still open. Leading guess: it carried a file, the class of message the stream never publishes (#415). The rescue line's file count settles it.

(b) The rescan did not find 1528 for 5 h 20 min. 192 of the window's 198 passes made 1 history listing and 0 thread listings. A pass lists a thread only when either:

- the history listing (top-level messages of the last `rescan_window_seconds`, 24 h by default, newest 5 pages) shows its root with a reply newer than its last listing, not all of them carried by the stream; or
- the stream carried a reply in the thread in the last 30 min (`active_threads`, `ACTIVE_THREAD_MILLIS`).

The thread's root (38.3 h old) was past the window, so no history row showed it. The stream's last reply in it before 1528 was 1526, 200 min earlier, so the thread was no longer active. The bridge's own 04:49:05Z post never counts: the stream never carries bot posts. So nothing made the thread due.

The stream carried 1527 at 10:10:29Z, which made the thread active. The pass at 10:11:04Z listed it ("2 listings found 1 message the stream did not carry") and rescued 1528. The ledger's `rescued` list holds exactly that message. This is why 1527 was numbered before 1528.

Two separate faults in the same window, not causes of the 5 h:

- The host listing budget held every bridge back after a rate limit on 47 passes (04:44Z to 09:14Z). Those passes listed nothing, but a normal pass would not have listed that thread either.
- From 09:46:19Z to 10:09:26Z, 28 reconnects failed with `[Errno 98] Address already in use` from the stream client; the 10:10:29Z reconnect worked on a new port. That loop delayed 1527 by about 8 min, and 1528 with it. It is filed separately as #2369.

# Design

Watched threads, durable, within the host listing budget:

1. The outbound helper records each thread the bridge posts or replies in (channel, root, time of its last post there) in a small durable file beside the rescan ledger. For a top-level post, the root is the post itself.
2. A rescan pass lists each watched thread from its listed-through point while the watch lasts: 48 h after the bridge's last post in it, configurable.
   - A thread whose root the history listing shows is still decided by that row, as today. So the extra cost applies only to watched threads with an older root.
   - Quiet watched threads back off: every pass for the first hour after the bridge's post, then at most every 10 min.
   - Watched-thread listings spend the host-wide listing budget like every other listing, after the owner channel's history, and each pass logs how many it made. If a hold or the per-minute budget stops a pass, the remaining watched threads stay due for the next pass.
   - Bound: at most 8 watched-thread listings per pass (the existing `MAX_THREADS_PER_PASS`), newest bridge post first.
3. The listed-through point of each watched thread is stored in the same durable file, not per subscription.

Interaction with the request-population refusal window (2048 cap, #2338):

- Today the rescan's thread state lives only in memory, per subscription. That state is `active_threads`, `threads` (listed-through points), `seen` and `postponed`; only the ledger is durable.
- A capacity refusal fails the batch before commit, so the provider drops the subscription and resubscribes from the saved cursor about every 60 s while the store stays full. Each resubscribe forgets which old-root threads were active, so a reply that only an active thread made visible becomes invisible again. A closed stream and a bridge restart do the same.
- The watched-thread record is durable, so it survives the reconnect loop and restarts. Because it keeps the listed-through points, a resubscribe does not re-list every watched thread from the window floor, which would spend the host budget once a minute.
- During the refusal window a rescued message is not lost. A refused rescued batch stays the ledger's one pending rescue and is offered again first, identical, by the next subscription (`open_rescan`). It is admitted once capacity frees, and under #2338 the hold is logged and alarmed.
- While a rescued batch is pending, no other rescued batch is offered (one at a time, the existing backpressure rule). A pass whose rescue queue is already full lists no watched threads, so a long refusal window costs no more than one history listing per interval.

Also:

- When the rescan rescues a reply in a watched thread, it logs the delay ("rescued N min after it was posted").
- (a) gets a note in the chat user guide: messages the event topic does not publish arrive only through the rescan, within one pass for a watched or recent thread.

# Acceptance Criteria

- A deterministic plugin test (fake transport, injected clock): a partner member's reply in a thread whose root is older than the rescan window, where only the bridge's app has posted recently, is rescued by the first pass after its grace period, with no stream activity in the thread. The test fails on the current code.
- A second test: the same reply arrives while the store is full (a refusal and resubscribe every minute). It is listed once, held as the pending rescue across the resubscribes, and admitted exactly once when capacity frees. Watched threads are not re-listed from the window floor at each resubscribe.
- Watched-thread listings stay within the host listing budget, are capped per pass, and are counted in each pass's log line.
- A watch ends 48 h (configurable) after the bridge's last post in the thread.
- The rule and its setting are documented in the chat user guide.

# Notes

- Reported on 2026-11-27 by the consumer team's lead through coordb; investigation lane cl-bridgeretire. The bridge's state and logs were read-only for agent-utils lanes, and the consumer team extracted them, redacted.
- Open for (a): the raw rescue line with its file count, and the plugin's configured rescan window line.
- Related: #2369 (the Errno 98 reconnect loop in the same window), #415 (file messages never reach the stream), #615 (stream activity pulls the next pass forward), #690 (rescan intake and the shared host budget), #769 and #1892 (relay poll), #2338 (request population cap).
