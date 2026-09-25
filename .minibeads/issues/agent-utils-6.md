---
title: 'audio-source-icons: distinguish device and agent reading modes'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- audio
- ui
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:02.654910125+00:00
updated_at: 2026-09-25T02:29:02.701648571+00:00
---

# Description

[gpt-5.6-sol] Keep the existing device icon for on-device speech and show a cloud icon whenever the configured remote voice agent is selected.

# Acceptance Criteria

The toggle changes icons immediately in both directions; accessible name and title remain accurate; browser tests cover both states.

# Notes

The current worktree adds separate device and cloud SVG states plus browser assertions. Land together with the device-speech changes after validation.

[gpt-5.6-sol] Closed after verification. The browser test now checks the initial remote-agent state and the synchronous switch to device audio: cloud then phone icon, `aria-checked`, exact accessible label, exact title, and no network request during the toggle. The full voice-page suite passes.
