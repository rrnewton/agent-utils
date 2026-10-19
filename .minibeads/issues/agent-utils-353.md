---
title: 'relay-echo-as-owner: the bridge''s echo of a relayed prompt is drawn as a "you" row and stored as the owner''s words'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-19T06:02:48.189712947+00:00
updated_at: 2026-10-19T06:02:48.189712947+00:00
---

# Description

[opus 5.5] A vibe-talk-v1 bridge echoes every prompt the page sends as a user transcript. The page recognises the echo of text the owner typed and of its own word on a send card, but not of a relayed prompt (the page's "Read these 2 new chat messages ..." turn that carries new channel messages into the call, #337 voice-relay-silent). So the echo of every relay is drawn in the call's transcript as a "you" row, as if the owner had said it, and stored as a turn in the owner's words. Seen live on 2026-10-18: two owner-turn records per relayed turn in one call. The stored conversation then claims the owner read the channel's messages aloud, and the recap a resumed (or, with #352 voice-reconnect-on-bridge-error, a reconnected) session is handed repeats them as the owner's words.

Fix: the echo of a relayed prompt is consumed like the echo of a typed one: no "you" row and no owner turn on the server. It answers for exactly one relay, so an owner who really says the same words still shows. The echo can arrive later than a typed echo's window (the bridge echoes when it accepts the prompt, which can follow a long answer), so the match must hold for as long as the prompt is the turn in flight rather than for a fixed few seconds. The hosted protocol is unchanged.

# Acceptance Criteria

- Page suite: the echo of a relay prompt draws no "you" row and stores no owner turn; the relay is still read out and the "not read out" accounting is unchanged.
- Negative controls: an owner's spoken turn with other words right after a relay is drawn and stored; a second identical echo (no second relay) is drawn; the echo of a typed message is handled exactly as before; a brokenScript that drops the relay from the echo match draws the row again.
- The echo of a relay re-sent by a reconnect is consumed on the new session too.
