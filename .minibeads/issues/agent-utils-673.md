---
title: 'voice-pace-control: grey read-aloud pace where the backend ignores it, and bring the slider back above the stepper'
status: open
priority: 2
issue_type: feature
depends_on:
  agent-utils-624: blocks
created_at: 2026-10-28T19:51:22.554459527+00:00
updated_at: 2026-10-28T19:51:22.559316096+00:00
---

# Description

[opus 5.5] Item 4 of #649 voice-agent-tools, held for #624 voice-page-modularity. Finding: ConversationSpeech (vibe-talk/src/speech.rs) ignores the speed argument (_speed), so read-aloud pace does nothing on the conversation backend (the internal stack); device voice, hosted vendor, Wit.ai and local TTS honour it. Plan: Description gains pace: bool per backend (chain: the first provider's), the page greys the Pace control with the reason where false, and the slider returns above the [-] [100%] [+] stepper (#567), both kept in sync. Owner 2026-10-28: 'does pacing work at all for our internal stack? If not let's detect it and grey the button. Also I want the slider for pace back and set above the current controls.'
