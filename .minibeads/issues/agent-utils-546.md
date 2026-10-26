---
title: 'read-aloud-silent-audio: a read that comes back as silence is reported as spoken'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
- voice
created_at: 2026-10-26T19:51:21.578921612+00:00
updated_at: 2026-10-26T19:51:21.578921612+00:00
---

# Description

[opus 5.5] When the private voice backend cannot synthesize speech (2026-10-26: its text-to-speech tier refused the request with an authorization error), the bridge still answers a read-aloud prompt with about 0.8-0.9 s of audio frames (20 binary frames, 38-42 KB of 24 kHz s16le PCM) followed by turn_complete. Every sample is silence. `ConversationSpeech` (vibe-talk/src/speech.rs) counts any binary frame as heard audio, so the read succeeds, the phone plays nothing, and the only trace is the absence of tap_to_speech_ms in the app log. The owner heard silence for a day before anyone looked.

Measured with a loudness probe (10 ms windows, RMS > 500): speech_ms=0 on every such read, against hundreds of ms for a real one.

# Acceptance Criteria

A read whose audio carries no speech (by a loudness measure over the PCM) is reported as a read-aloud failure with a distinct code (for example conversation_speech_silent), is logged, and is retried or falls back the way other no-audio failures do; the page says the read failed instead of playing silence. A unit test feeds a turn of silent PCM plus turn_complete and expects the failure; a real read still passes.
