---
title: 'read-pause-reply-resume: Resume after pausing a read, replying and sending says nothing on the device voice'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-10-26T19:48:06.883273655+00:00
updated_at: 2026-10-26T19:48:06.883273655+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26, in a car with the device voice: reading a long message, pause, Reply on that message, dictate the reply with the keyboard's microphone, Send, then Resume on the Read button: nothing was heard. Cause: Chrome on Android has no speech pause. Chromium's TtsPlatformImplAndroid::Pause calls StopSpeaking and Resume is empty, and the stopped utterance is never ended, so the engine still counts it as current. The page paused with speechSynthesis.pause() and resumed with speechSynthesis.resume(), which was silence until the 60 s watchdog gave the read up. Fix: on every engine the device voice's pause is a cancel at the sentence, and Resume says that sentence again from its start and goes on; the device voice says one sentence per utterance (short ones joined to the next, long ones still cut at 180 chars).

# Acceptance Criteria

Page tests: Pause cancels and never calls the engine's pause, Resume re-speaks the paused sentence, no watchdog fires while held, the read finishes and is archived. A Chromium walk at 393x852 touch, for both voices, with a device voice that behaves like Chrome on Android: read the long message, pause in its second sentence or block, Reply, type, Send, Resume. The read must continue from the paused sentence (device voice) or with the audio clock moving (agent voice). The walk fails on the page before the fix.
