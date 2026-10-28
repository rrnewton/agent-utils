---
title: 'voice-reconnect-heal: a voice call survives a phone''s network changes without losing the call'
status: open
priority: 1
issue_type: bug
depends_on:
  agent-utils-624: blocks
created_at: 2026-10-28T19:51:17.749934164+00:00
updated_at: 2026-10-28T19:51:17.752751641+00:00
---

# Description

[opus 5.5] Follow-up of #649 voice-agent-tools (f), page side, after #624 voice-page-modularity. Owner car test 2026-10-28 13:46-13:50 ET: 'voice relay: socket closed caller=Page' then open/close churn every 3-9 s. Server side landed in caabe59c: the relay's close line now says ended=client_close|client_gone|client_silent|bridge_close|bridge_gone, code, secs and frame counts. Next: read those lines from the next car test to tell a network drop (client_gone, short lives) from a page reconnect loop (client_close 1000/1001 from a page that did not hang up); then make the page's reconnect heal (#275 voice-auto-reconnect, #357 voice-reconnect-on-server-close, #352): one reconnect at a time with backoff, the call's record and pending turn carried, no loop.
