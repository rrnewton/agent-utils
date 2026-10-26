---
title: 'wit-tts-provider: a Wit.ai voice for read-aloud'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
- voice
created_at: 2026-10-26T20:32:35.117291686+00:00
updated_at: 2026-10-26T20:32:35.117291686+00:00
---

# Description

[watermelon-full-preview] Owner direction 2026-10-26: use an already-running public speech service, no GPU, no self-hosting. Wit.ai's public speech API fits: POST https://api.wit.ai/synthesize (Bearer app token, versioned ?v=, JSON {q, voice, style?, speed?, pitch?}, Accept audio/wav) and GET /voices for the Settings picker. Today cloud read-aloud uses a private WebSocket voice stack that is ACL-bound and down; device voice and ElevenLabs are the other options. Build a Wit.ai TTS provider plugging into the same read-aloud flow (per-chunk synthesis, prefetch, pause/resume, scrub bar): [read_aloud] backend = "wit", token read from a file path (never inline, never logged), plus voice and speed. Settings gains a Wit.ai voice choice when configured. Errors are loud (401/403, 429 with backoff then a clear message, network errors, silent output stop the read with an on-page message; never fake playback). Sentence-sized requests, bounded concurrency (1-2), small cache keyed by (text, voice, speed). Tests: a fake Wit server (success, 401, 429, slow, silent audio) and a page walk at 393x852 and desktop against the fake. Live check waits on an owner-created Wit app token file.

# Acceptance Criteria

Backend wit synthesizes via the fake Wit server and plays through the existing ticket flow; 401/403, exhausted 429, network failure and silent audio each stop the read with a distinct clear message; voices endpoint lists fake voices and Settings shows the Wit.ai voice choice plus picker; token comes only from a file and never appears in logs or errors; validation gate passes.
