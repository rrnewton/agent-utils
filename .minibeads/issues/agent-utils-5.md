---
title: 'device-speech-e2e: close the Android speech-output regression loop'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- audio
- android
- testing
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:02.484760695+00:00
updated_at: 2026-09-25T02:29:02.701648571+00:00
---

# Description

[gpt-5.6-sol] Reproduce the reported on-device speech noise or wrong-language output, prevent selection of an incompatible local voice, and provide an opt-in device harness that captures audio and evaluates audibility plus language or transcript correctness without storing private message text.

# Acceptance Criteria

Offline negative controls pass; an attached Android run records actual emitted speech and rejects silence, corrupt audio, wrong language, or poor transcript overlap; normal validation remains hermetic.

# Notes

A work-in-progress patch now rejects local voices whose language does not match navigator.languages, adds browser regression tests, and adds an opt-in ADB capture plus external-STT harness with offline negative controls. This host has ADB and ffmpeg but no attached device, emulator SDK, or STT adapter, so the physical closed loop remains unverified.

[gpt-5.6-sol] 2026-09-25 external-device blocker: `adb devices -l` and `adb mdns services` both found zero Android targets. The host has `/dev/kvm` and bare `qemu-system-x86_64`, but no `emulator`, `avdmanager`, or `sdkmanager` executable, no `~/.android/avd`, no Android SDK in the standard local or system locations, and no cached Android system image in the searched local installation/cache paths. It also exposes no `/dev/snd`, installed audio capture command, STT executable, or cached standalone STT model. A direct harness attempt therefore stopped before browser startup with exit 30: `expected exactly one authorized Android device, found 0`. The hermetic evaluator still passes all nine controls. Completion requires an authorized Android Chrome target, working playback capture (or a capture adapter), and an STT adapter; no actual speech was synthesized or claimed as verified here.
