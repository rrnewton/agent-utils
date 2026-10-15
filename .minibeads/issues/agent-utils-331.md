---
title: 'resume-on-websocket: make resuming honest, show what a new call was told, and support it on the websocket backend'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T15:52:38.617304322+00:00
updated_at: 2026-10-15T15:52:38.617304322+00:00
---

# Description

Owner, 2026-10-15: 'Tell a new call what was already said' (#46 conversation-replay) is a live checkbox even when the server cannot resume, and Settings only says 'Resuming is OFF on this server' in small text; with the websocket conversation backend replay is always off because that backend has no frame for it. (1) When the server cannot resume (replay.enabled false, or a backend that cannot carry a replay), show the switch disabled with the reason beside it, never as a control that does nothing. (2) Make resuming work on the websocket conversation backend: define a frame that hands the new call the written record of the earlier one (the same reconstruction the ElevenLabs path sends, within replay.max_chars / max_turns), document it in the bridge protocol so a bridge can feed it to its model, and report replay as available only when the bridge says it accepts it. (3) A visible indicator when a call starts: how much earlier conversation the agent was given (for example 'Caught the agent up with 1,240 words from earlier'), or why it started fresh, on the call's view as well as in Settings. Tests: page tests for the disabled state and the indicator; a protocol test against the loopback bridge.
