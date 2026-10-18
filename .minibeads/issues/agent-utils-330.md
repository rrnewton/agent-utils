---
title: 'voice-long-turn-drop: a long spoken summary is not shown as text and the call ends midway'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.613292169+00:00
updated_at: 2026-10-18T15:51:00.071360545+00:00
closed_at: 2026-10-18T15:51:00.071360455+00:00
---

# Description

Owner, 2026-10-15, Android installed app, voice call on a deployment whose voice runs through the websocket conversation backend: asked the voice agent for a summary of overnight messages. The agent said one short line ('Checking overnight messages now.'), then began a long spoken summary. The long summary never appeared as text in the call's transcript, and the call ended by itself midway through the narration, going back to the idle state with no reason shown. Expected: every spoken agent turn appears as text as it is spoken (or at least when it ends), a long turn does not end the call, and if a call does end the page says why (#268 voice-call-drops made calls say why they ended). Investigate from the server's and the voice bridge's logs for that call which side ended it (a turn or message size limit, a timeout on a long tool result or turn, the transcript frame for a long turn never sent, or the socket closing), fix the vibe-talk side, and write down what a bridge must do if the cause is there. Tests with the loopback voice fake: a long agent turn after a tool call is shown as text and does not end the call.
