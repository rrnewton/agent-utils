---
title: 'voice-relay-silent: a new message in a watched channel is not relayed into the call'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T19:03:00.279530093+00:00
updated_at: 2026-10-15T19:03:00.279530093+00:00
---

# Description

[opus 5.5] Owner, 2026-10-15 about 13:00-13:20 ET: during a long voice call, with Live messages set to read every new message out in full, a new message arrived in a watched channel (the provider's own phone notification showed it), but the voice agent said nothing and the logs show nothing was delivered to the agent. Find where the relay stops: is the live event received by the page for that channel (watched but not the channel in view?), is it queued and never released (the hold-during-a-long-answer path that #330 voice-long-turn-drop's investigation found caps at 30 s and then interrupts), is the relay limited to the open channel, or does the bridge drop a mid-call injected message? Note: the server that should have carried the call shows no voice session at that time, so first establish which deployment the phone's installed app opens. Fix the page so every new message in a watched channel is relayed under the 'read out in full' setting, or the page says why not; never silently drop one. Page tests with negative controls for: a message in the open channel, in another watched channel, during a long answer, and while the call is idle.
