---
title: 'local-tts-provider: a local CPU voice for read-aloud'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
- voice
created_at: 2026-10-26T22:07:46.156399043+00:00
updated_at: 2026-10-26T22:07:46.156399043+00:00
---

# Description

[watermelon-full-preview] Owner, 2026-10-26: run TTS locally on the devserver alongside vibe-talk so reliable message reading (with block navigation) does not depend on a cloud service or the private voice stack. A read-aloud provider that runs an open-source TTS model locally on CPU as a sidecar next to vibe-talk: Kokoro-82M (Apache-2.0) first, Piper (MIT) as the light option. No GPU, no network at synthesis time. Model files fetched once into a configured directory outside the repo; weights and voices stay out of git; licences checked per voice. Measure per-sentence latency and real-time factor on this devserver's CPU. Same loud-error (explicit error or silent audio stops the read) and block-navigation behaviour as the Wit provider (#552), and designed so read-aloud-fallback-chain can order local, Wit, websocket and device providers. Ask the coordinator before any step that writes more than 10 GB. Stacked on #552 wit-tts-provider.

# Acceptance Criteria

Backend local synthesizes on CPU with no network at synthesis time through the existing ticket/block flow; a sidecar that is down, slow or silent stops the read with a distinct clear message; per-sentence latency and real-time factor measured on this devserver and recorded; model dir is configured, outside git, with per-voice licences checked; tests use a fake sidecar plus a real-model smoke when weights are present.
