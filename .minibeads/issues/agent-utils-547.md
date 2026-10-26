---
title: 'car-mode-design: a car view for reading, pausing, replying by voice and resuming, with big targets'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
- ui
created_at: 2026-10-26T19:55:14.402391860+00:00
updated_at: 2026-10-26T19:55:14.402391860+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26, after #554 read-pause-reply-resume: pause, Reply, the keyboard's microphone, Send and Resume take too many presses on targets too small for a car. Read should probably switch a phone into a full car mode (the Zoom precedent: a swipe to a car view with one big tap-to-speak button). It must be easy to pause, dictate a reply to the message being read, and resume where the read was, and replies may be several small ones sent as a batch. Request 219 adds: on mobile, a full-screen view of the message being read, its scrolling confined to that message, the other messages back when the read is paused or stopped. The proposal is ai_docs/transient/2026-10-26-car-mode/README.md: entry, dictation (keyboard, Chrome SpeechRecognition on-device where offered, or the server's voice path), send with readback and Undo, auto-resume, Media Session for both voices, the full-screen sheet (car view first; pause keeps it, stop closes it; Back exits; follow-the-sentence with a hand-scroll pause; none on a desk), how it fits channel-bars-layout, phases, and questions for the owner.

# Acceptance Criteria

The owner answers the proposal's questions; then phase 2 (car view shell with keyboard dictation and auto-resume) gets its own issue.
