---
title: 'browser-audio-output: isolate headless checks from unavailable host audio'
status: in_progress
priority: 1
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:10:16.956753038+00:00
updated_at: 2026-10-27T18:10:16.965094629+00:00
---

# Description

[GPT-6 Codex] Confirmed pre-existing host-audio failure. Implemented a shared headless-audio fixture that uses Chromium's --disable-audio-output test sink, replacing only the final OS stream while preserving decoding, scheduling, playback clocks and media-element buffering. All four affected branch steps pass with the ordinary host environment and their original assertions/negative controls. The earlier clock-assertion adjustment was removed. Related historical evidence: #234 oss-redeploy-check. Track completion through agent-utils-f0a72c vibe-voice-validation.

Baseline: pristine origin/main 1f0a7a902278276ca3a76badfd26f8c2af29ea61, tracked validation.dag.yaml, one active step, two CPUs, 4 GB, nice 15, idle I/O. Four original browser steps failed: streamed read/recovery remained suspended, blocks stayed at 1 of 7, views advanced the audio clock only 0.012 seconds in 300 milliseconds, and fallback speech took 5,579 ms (5,030 ms before play and only 19 ms to load). Page shard 2 and sent-receipt expiry passed in that baseline run.

Controlled environment repair: set PULSE_SERVER=unix:/tmp/vibe-voice-unavailable-pulse only for headless validation, avoiding the host's unreliable default WSL audio socket. No application source, media decoder, silence guard, clock-progress assertion, latency limit or negative control changed for this experiment. All four ORIGINAL main audio steps then passed. The media-element negative control remained slow at 4,773 ms, while streamed playback started in 569 ms and switched in 549 ms.

The feature branch's matching six-step run then passed all six: page shard 2, views, blocks, fallback chain, sent receipts and streamed recovery. Streamed playback was 565 ms, switching 548 ms; the media-element control remained 4,771 ms. This establishes that the original four audio failures were not caused by the feature branch. It does not prove physical-speaker audibility; the owner performs that deployment check.

The full selected gate remains required before publication, with the same test-only audio override and constrained low-priority scheduling. Do not change the deployment's audio environment on behalf of this test repair.

Portable-fixture evidence: four original audio steps passed in 119.6 seconds without PULSE_SERVER override. Python strict typing passes for all 91 source files. The shared fixture is used by streamed read-aloud, blocks, views and fallback-chain checks. The media-element and slow-voice negative controls remain discriminating. The complete selected gate is next.
