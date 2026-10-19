---
title: 'voice-reconnect-on-server-close: a vibe-talk-v1 close whose reason asks the client to reconnect still ends the call'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- voice
created_at: 2026-10-19T08:30:18.188057386+00:00
updated_at: 2026-10-19T08:30:18.188057386+00:00
---

# Description

[opus 5.5] Follow-up from #352 voice-reconnect-on-bridge-error, which carries a call on a new session when a vibe-talk-v1 bridge sends an error frame asking the client to reconnect ("turn N never started after an interrupt; reconnect"). A bridge also closes a session with a reason in the same family: code 1011, "interrupt boundary timed out; reconnect". The page records that as server_closed and ends the call, with the red panel. Decide whether such a close gets the same treatment, under the same bound (one automatic reconnect per rolling minute, at most three per call), with the same records (reconnected, reconnect_limit, reconnect_failed) and the same "not read out" accounting. A close names no turn, so nothing would be sent again: a relayed turn in flight is said not to have been read out, and a typed one is left to the reader.

# Acceptance Criteria

- Page tests: a 1011 close whose reason ends "; reconnect", on a session that started, keeps the call on a new session, records reconnected, and says so; nothing is sent again, and a relayed turn in flight is said not to have been read out once.
- Negative controls: a close with any other reason, a close before the session started, a close while the page is in the background (still a suspension), and a close inside the bound still end the call as today.
- The loopback Chromium walk covers it.
