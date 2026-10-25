---
title: 'voice-greeting-wording: the voice agent must not greet with "connect you to your voice assistant"'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-511: related
created_at: 2026-10-25T15:36:02.360633546+00:00
updated_at: 2026-10-25T16:18:08.886465841+00:00
---

# Description

[opus 5.5] Owner report 2026-10-25: on connect the deployment-managed voice agent says "let's connect you to your voice assistant/agent", wording carried over from the hosted agent, which makes no sense when the voice agent itself is speaking. The sentence is a constant in the deployment-managed bridge, not in this repository; vibe-talk can only steer it from the system prompt it serves, and the bridge needs a one-line change (or to take the greeting from the profile).

# Design

[opus 5.5] The sentence is INITIAL_GREETING_PROMPT in the deployment-managed bridge (realtimeai/cosmos/wearables_playground/src/main.rs:84): "Say exactly this sentence and nothing else: Hello, let's connect you to your voice assistant." It is sent as a hidden text turn on the first audio_start of every session, reconnects included. Bridge fix (one line): "Say exactly this sentence and nothing else: Hello, you're connected to your chat." Better: Profile gains #[serde(default)] greeting: Option<String> and the bridge says the profile's greeting when present; vibe-talk then serves greeting in /api/v1/voice-agent. Stopgap in vibe-talk prompt v13 (#511 commit): the base prompt tells the agent it is the voice agent, never to speak of "your voice agent/assistant" as someone else, and to answer that scripted hello with "Hello, you're connected to your chat." Unverified against the live model.
