---
title: 'voice-reconnect-on-bridge-error: keep a call when the voice bridge loses a turn and asks the page to reconnect'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-19T06:02:48.130859443+00:00
updated_at: 2026-10-19T08:30:37.272674841+00:00
closed_at: 2026-10-19T08:30:37.272674731+00:00
---

# Description

[opus 5.5] Seen live on 2026-10-18 (build 4a18509c, a vibe-talk-v1 websocket bridge). The owner's spoken turn was cut by the bridge's own barge-in (turn_complete interrupted:true), or in a second run completed with half a second of audio and no text. 1.2 s later the page relayed waiting channel messages: audio_end, then one prompt carrying two messages. The bridge echoed the prompt and never started its turn. 10 s later it sent {"type":"error","message":"turn 4 never started after an interrupt; reconnect"}. The page treats every error frame as the end of the call: it sent audio_end and quit, closed the socket, and showed "The voice service reported an error ... and the call has ended". The two relayed messages were never read out, and nothing said so: the transcript and the connection details carried no "not read out" line.

The bridge defect itself (after an interrupt the next turn never starts; the bridge's watchdog reports it after 10 s and asks the client to reconnect) was recorded in agent-utils-7 as D2 and belongs to the voice service. The server's read-aloud path already survives it by retrying on a fresh session. The call page does not.

Two defects on the page side:

1. A relayed turn that was sent and never answered when the call ended is dropped silently. The "not read out" report of #337 voice-relay-silent counts only messages still queued; it misses the turn in flight. (The hosted protocol drops its relayed turn in flight the same way.)
2. An error that asks the client to reconnect ends the call, although the bridge says how to recover.

Proposed:

- Report a relayed turn the call ended before answering as not read out, on both protocols.
- On vibe-talk-v1, an error frame whose words end in "; reconnect", once the session has started, does not end the call. The page:
  - says goodbye to the old session and asks the server for a fresh voice session;
  - keeps the microphone (no second permission prompt), the audio, the conversation and the relay state;
  - opens a new socket;
  - hands the new session this call's record through the #331 resume-on-websocket replay frame where the session lists replay, the server offers it and this device's Resuming switch is on; otherwise it starts the new session fresh and says so, including that the new session has never seen what was said above;
  - sends the turn that never started again, exactly once, and only when the bridge named that turn and no answer to it had arrived. Each relayed message still reaches the agent once; a typed message once, and its echo is not drawn twice. The page's own word on a send card goes again only when the recap was taken;
  - says "reconnected after the voice service lost a turn" in the transcript, the line under the call, the status line, the connection details and the stored record.
- Bounded: at most one automatic reconnect per rolling minute and at most 3 per call. A request beyond either ends the call as reconnect_limit, with a sentence that says why. A reconnect that cannot get a working session ends the call as reconnect_failed.
- The call-end record gains reconnected (one per replaced socket), reconnect_limit and reconnect_failed, and a reconnects count.
- Nothing else reconnects: not a hang-up, not a drop, not any other error, not an error before the session started, and not the hosted vendor's protocol. This is a narrow exception to "nothing auto-reconnects" and to #275 voice-auto-reconnect: the call never ended from the reader's side, the microphone was never closed, and the bridge asked. It also happens with the page hidden, since the call and the microphone are already live.
- Out of scope, follow-ups: a server close whose reason ends "; reconnect"; a structured reconnect field on the error frame; a recap that also carries the conversation an earlier resume handed the first session.

Related: #353 relay-echo-as-owner, #354 live-relay-check, #268 voice-call-drops, #275 voice-auto-reconnect, #330 voice-long-turn-drop, #331 resume-on-websocket, #337 voice-relay-silent, agent-utils-7 (D2).

# Acceptance Criteria

- Page suite:
  - a reconnect-asking error keeps the call: a second session and socket, the replay first where it applies, the lost relay turn sent again once, the transcript says why and what the new session knows, and the call-end records are [reconnected, hang_up] with reconnects 1 on the second;
  - the shape a deployed bridge sends (a greeting, no replay): no replay frame, the greeting once, the lost turn once after it, the line says the session started fresh;
  - a typed lost turn goes again once, with one row and one record;
  - an answered turn is not sent again;
  - a message arriving during the reconnect waits and goes once;
  - spoken yes is disarmed across the reconnect;
  - the microphone goes to the new socket only, with no second getUserMedia;
  - the old socket closing right after the error does not end the call;
  - negative controls: a non-reconnect error still ends the call as error_frame with no second session; a reconnect-asking error before the first session started ends it as error_frame; a second reconnect request within the minute, and a 4th in one call, end it as reconnect_limit; a reconnect whose session cannot start (mint refused, socket refused, no open in time, closed or erred before session_started) ends it as reconnect_failed; Hang up during a reconnect opens nothing more; "not read out" counts are exact (2 messages, never 4).
- An unanswered relayed turn at call end is reported "not read out", on both protocols. Its control: an answered one is not.
- The hosted protocol is unchanged: its error frames still end the call.
- A loopback Chromium walk with a fake bridge that sends this error passes, and fails against the page before the change.

# Notes

[opus 5.5] Delivered on branch vt/voice-reconnect in "vibe-talk: a relayed turn the call ended before answering is said not to have been read out" (both protocols), "vibe-talk: pull the vibe-talk-v1 call socket's wiring out of the start, unchanged", "vibe-talk: record a call carried on a new session, or ended for lack of one" (reconnected, reconnect_limit, reconnect_failed and a reconnects count), "vibe-talk: when the voice bridge loses a turn and asks to reconnect, carry the call on a new session", "vibe-talk: check a lost turn's reconnect in Chromium against a bridge that sends the error", and "vibe-talk: a lost turn already sent again once is not sent a third time" (a review found a turn sent again could go once more when the new session lost it over a minute later). The bound is one automatic reconnect per rolling minute and at most three per call; the device's Resuming switch is honoured; a reconnect also happens with the page hidden. The plan's review corrections are all applied: the turn in flight is let go before it is queued again, so a later end counts its messages once; the microphone graph is rebuilt on the new socket before anything is sent; the deployed shape (a greeting, no replay) is the main path of the page tests and the walk; the reconnected line says what the new session knows; every failure of the new socket before its session starts ends the call as reconnect_failed, a suspension excepted; the lost socket is let go inside the error's own handler, before the close a bridge sends after it; the page's word on a card goes again only to a session that took the record; the old turn's health is reset; output before the echo does not count as the lost turn's answer.

Not verified here: no run against a real bridge from this lane. On the bridge it was seen on, which lists no replay, a reconnected session starts with no context, and says so. Follow-ups: #357 voice-reconnect-on-server-close; a recap that also carries the conversation an earlier resume handed the first session (the details say it was not sent again); a structured reconnect field on the error frame (not asked of the bridge owner now).

[opus 5.5 coordinator / claude worker] Review rounds 2 and 3 added "vibe-talk: a reconnected session that refuses the record is said so in the stored record too" (a reload no longer contradicts the live line; the recap wording is "was sent this call's record") and "vibe-talk: in a typed call a reconnect's greeting is waited for before the lost turn goes again" (sent into the greeting, a typed call's lost relay could be let go unread and its echo drawn as the reader's words). Left open, each needing a bridge that breaks the protocol or a rare shape: a relayed turn whose turn_complete carries no answer at all is let go with no "not read out" (older than this issue); PCM after the echo followed by "turn N never started" for that same turn neither resends nor reports; a reconnect's socket that opens but never starts a session keeps "Reconnecting…" up until Hang up; in a typed reconnect whose promised greeting never comes, messages stay queued until Hang up, with "The voice service is not responding." shown; PCM counts as an answer only after the bridge's echo, so a bridge that never echoes gets a cautious "not read out" on a hang-up mid-answer.
