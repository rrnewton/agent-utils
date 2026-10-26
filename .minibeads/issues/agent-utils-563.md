---
title: 'read-aloud-fallback-chain: ordered voice providers with per-block fallback'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
- voice
created_at: 2026-10-26T22:32:35.703545036+00:00
updated_at: 2026-10-26T22:32:35.703545036+00:00
---

# Description

[watermelon-full-preview] Owner, 2026-10-26: two options on TTS so cloud/local reading keeps working even if one voice stack is down. Configurable ordered read-aloud providers (e.g. local, then wit, then the websocket voice stack, then device). On an explicit error or silent audio for a block, the next provider takes the SAME block; the page shows which voice is speaking; Settings shows the order. Depends on #552 wit-tts-provider and #561 local-tts-provider, whose SpeechProvider interface (speak/speak_stream_voiced/voices/describe, typed SpeechError codes) was designed for this: a composite provider can order backends and fall through per error code without page changes beyond naming the speaking voice.

# Acceptance Criteria

Config names an ordered provider list; a block that errors or is silent on one provider is spoken by the next and the page names the voice that spoke it; when every provider fails the read stops with the last clear error; Settings shows the configured order; tests cover fall-through on error and on silence with fake providers.
