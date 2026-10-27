---
title: 'cloud-read-failures: a read-aloud the voice service answers with silence or an error stops and says why, never reads on in silence'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-10-26T19:58:57.752072313+00:00
updated_at: 2026-10-27T01:06:40.526894054+00:00
closed_at: 2026-10-27T01:06:40.526893984+00:00
---

# Description

[opus 5.5] Owner request 220, 2026-10-26: with the voice service's speech access expired, the page opened the scrubber and read through the message in silence. The bridge sends no error frame on a TTS failure: about 0.85 s of 24 kHz 16-bit PCM whose every 10 ms window has RMS under 500, then turn_complete (measured on the private stack the same day). Required: stop the read on any error or failure for it, close the scrubber, and show the cause in short words (raw reason as fallback); a guard that stops a chunk with no speech within about 4 s, saying the cloud voice is not producing audio, with Read with device voice for that message; and the failure logged on the server and visible in diagnostics. #540's silence guard stopped only once a whole part had arrived, by a peak level, with no cause and no way on.

# Acceptance Criteria

Server: silent turns logged at WARN; a provider error frame's optional code carried; POST /api/v1/speech/{ticket}/failure (ticket-authorised like timing) logs and records the last failure, which diagnostics shows. Page: loudness by 10 ms RMS windows; silence or no speech within 4 s stops the read with Cloud voice: ... and Read with device voice; an error answer's detail shown in short words. Page tests, Rust tests, and the browser walk's silent control.
