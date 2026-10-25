---
title: 'voice-self-hearing: the voice agent hears its own greeting and answers it as the owner'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
depends_on:
  agent-utils-511: related
  agent-utils-513: related
created_at: 2026-10-25T22:36:44.166763604+00:00
updated_at: 2026-10-25T22:58:23.360398040+00:00
---

# Description

[opus 5.5] Owner report 2026-10-25 (15:13 PDT): on the phone, Start a new call; the agent said "Hello, let's connect you to your chat.", then HEARD "Let's connect you to your chat" as a message from the owner and answered it. The agent's audio plays through the phone speaker and the microphone picks it up; the page does nothing to stop that reaching the voice service as the owner's speech, nor to stop the page filing it, and reporting it to #511 voice-heard, as the owner's words.

# Design

[opus 5.5] Diagnosis. The call itself is in no log this harness can read: neither deployment's voice bridge had a session in the reported window (only its watchdog's probes), and the one voice session minted then stopped before it opened a socket. So the cause is established from the page, not from the call. Ruled out: the bridge's hidden greeting instruction filed as the owner's turn: the words heard were the agent's spoken output, and the agent answered them, which only input reaching the voice service can cause. The page's part, three faults: (1) on vibe-talk-v1 the greeting gate applied only to a session that promised a greeting (greeting: true), and ended 750 ms after the greeting's turn_complete, which the bridge sends when it has SENT the greeting, while the phone may still have seconds of it queued; the hosted agent's first message had no gate at all; (2) agent audio plays through Web Audio, which a phone browser need not give its echo canceller as a reference, and a refused echoCancellation constraint changed nothing about capture; (3) a user transcript of the agent's own words was drawn, filed and reported to /api/v1/voice-heard (#511) as the owner's. Fix (web/voice.js): every voice session holds the microphone while whatever its agent says first is audible (last scheduled sample + reported output latency + ECHO_TAIL_MS), until a promised greeting's turn_complete has come or, for any other, the speaker has been quiet ECHO_GREETING_QUIET_MS (a stall is not the end); the owner speaking while the agent is silent ends it. With echo cancellation off or refused the microphone is held whenever the agent is audible (half duplex, said in the connection details). Transcript judge: only when the microphone actually sent a buffer while the agent was audible, a user turn whose first frame comes within ECHO_PARTIAL_GRACE_MS (hosted finished turns: ECHO_FINAL_GRACE_MS) of that, made (at least 80%, in order) of the words of one of the assistant's latest ECHO_TURNS turns, is an echo: not drawn, filed, reported as heard or judged as a yes. Begun after the agent fell silent, a turn is judged only from three words and never when it is a plain yes; until it can be judged nothing of it is reported, so an echo's opening words never reach the server. Known trade-off: the owner repeating a question's words as a send ask within ECHO_PARTIAL_GRACE_MS of the agent's last audible sound is taken for an echo; the send is refused and he asks again. Ask for the deployment-managed stack: (a) the voice service accepts and transcribes input audio while still sending its own output; a page cannot unsend what it already streamed; suppress or flag input that correlates with the session's own output, at least during the scripted greeting; (b) make the scripted greeting the reviewed sentence (#513): the model blended the scripted line with the prompt's correction into 'let's connect you to your chat'; (c) say greeting: true in session_started when it greets (the page now holds the microphone either way, but a promised greeting ends cleanly at its turn_complete). Follow-up not done here: routing playback through a loopback RTCPeerConnection so Chrome's echo canceller references it; it needs a physical-device test.
