---
title: 'voice-stack-turn-taking: end-of-turn silence and dropped input transcripts in the deployment-managed voice stack'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-512: related
  agent-utils-513: related
created_at: 2026-10-25T17:46:09.235410158+00:00
updated_at: 2026-10-25T17:46:09.235410158+00:00
---

# Description

[opus 5.5] Owner action, not vibe-talk code: two changes the deployment-managed voice stack needs, found in the 2026-10-25 diagnosis of #512 voice-recap-private-stack (details in that issue's comment of 2026-10-25). (1) End-of-turn silence: the model session runs server VAD with silence_duration_ms unset; measured pauses of 0.41 s and 0.50 s split one spoken sentence into separate turns. Wanted: silence_duration_ms of about 1000 ms (900-1200), or semantic VAD. (2) Dropped input transcripts: the orchestrator drops user input transcripts for speech that begins after a turn completed and before the next starts (18 of 25 completed user transcripts in one host's log); the model still hears that speech, so the page's transcript diverges from it. Wanted: forward input transcripts regardless of turn validity. vibe-talk already works around the worst effect of (2) since #512: the page holds its own messages while the microphone hears the owner. The greeting wording is #513 voice-greeting-wording. Close this when the stack's owners have made or declined both changes.
