---
title: 'voice-recap-private-stack: hand a reconnected or resumed call its recap on the deployment-managed bridge'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-511: related
created_at: 2026-10-25T15:36:02.357412382+00:00
updated_at: 2026-10-25T17:46:19.100384989+00:00
closed_at: 2026-10-25T17:46:19.100384849+00:00
---

# Description

[opus 5.5] Owner report 2026-10-25: after a lost turn the call reconnected and said "this voice session takes no recap". The deployment-managed vibe-talk-v1 bridge never lists replay in session_started, so every reconnect starts fresh. The bridge fetches GET /api/v1/voice-agent for every session (reconnects included) and uses prompt.text verbatim as the system prompt, so the server can hand the next session the recap there without a bridge change. Also record why the turn was lost: the page sent audio_end for a relayed chat message while the owner was still speaking, because the speech runtime dropped his transcripts and the page judges speech only from transcripts.

# Design

[opus 5.5] Diagnosis (2026-10-25 call, deployment-managed stack logs): (1) the owner said "Hold on. Let me interject some responses here..." over a long summary; the speech runtime dropped those user transcripts ("dropped output from an invalidated turn": it drops input transcripts that arrive after a turn completed and before the next starts; 18 of 25 completed user transcripts in that log were dropped). (2) The page judges "owner speaking" only from transcripts, saw none, and sent audio_end mid-sentence to deliver a queued relay as a text turn. (3) The model service took the text item but never started a response (VAD still in speech, input ended); after 10 s the page reconnected. (4) The bridge lists no capabilities in session_started, so the page sends no recap.

Smallest fix with no bridge change: the bridge fetches GET /api/v1/voice-agent for EVERY session, reconnects included, and uses prompt.text verbatim as the system prompt. So: (a) POST /api/v1/voice-recap {conversation_id} (write scope) builds the recap with the same function as /conversations/{id}/replay and arms it one-shot for 60 s; (b) /api/v1/voice-agent appends an armed recap to prompt.text as fenced quoted data (fingerprint of the text actually served) and marks it consumed, except while this server's own BridgeHello probe is in flight (that probe also opens a bridge session); (c) GET /api/v1/voice-recap answers armed/consumed/none; (d) the page, on a vibe-talk-v1 session without replay, arms before opening the reconnect (or resumed) socket and, after session_started, marks the recap sent with transport "profile" when consumed, otherwise "unsupported" as today. The bridge Profile struct does not deny unknown fields.

Alternative bridge change: list "replay" in session_started.capabilities and handle {type:"replay", text, ...} by appending text to the session system prompt (or a hidden conversation item) and answering replay_ack.

Separately for the drop: the page should hold page-originated prompts (relays, notices) while the microphone shows local speech energy (it has none today; capture is a ScriptProcessor, so an RMS per 4096-sample buffer is cheap), capped so road noise cannot starve relays; and the speech runtime should forward input transcripts regardless of turn validity.
