---
title: 'voice-approval-browser: stabilize playback readiness in the boxed call walk'
status: in_progress
priority: 2
issue_type: bug
assignee: codex-vibe-voice
created_at: 2026-10-27T18:12:27.831038361+00:00
updated_at: 2026-10-27T18:23:56.682951848+00:00
---

# Description

[GPT-6 Codex] Confirmed pre-existing browser-fixture race on clean main b19c1406 under two active steps/eight CPUs/32 GB/nice 15/idle I/O. The fixed 2.5-second sleeps sometimes precede the queued read-back finishing; the application correctly refuses the simulated yes with “the yes came before the read-back finished playing”. Failure-state logging reproduced that refusal in two of six clean-main call walks. A third-call variant also missed its Heard yes state. No application approval defect is established.

The fixture now waits for numbered read-back completion and both wall/audio playback clocks, within the SAME original 2.5-second budget. Only then does it send its simulated yes. All approval assertions, echo protection, actual audio scheduling and late-transcript ordering remain intact. Comparison logs live with the coordinator handoff outside this repository. Track selected-gate evidence with agent-utils-f0a72c vibe-voice-validation.
