---
title: 'voice-smoke-cadence: smoke-test real voice conversations on an adaptive cadence'
status: open
priority: 1
issue_type: bug
labels:
- voice
- vibe-talk
- ops
created_at: 2026-10-27T12:46:36.873314992+00:00
updated_at: 2026-10-27T12:46:36.873314992+00:00
---

# Description

[opus 5.5] 2026-10-27 ~04:20 PDT the owner's conversational voice call from the phone page failed 0 s in: "The voice service reported an error 0 s into the call, and the call has ended: speech synthesis failed: ExceptionGroup: unhandled errors in a TaskGroup (1 sub-exception)". Read-aloud kept working, and so did the private watchdog's hourly read-aloud audio probe (speech_ms 340-490 all night), so nothing noticed.

Cause (upstream of vibe-talk): the private voice backend's development configuration points at a language-model tier and a speech-recognition tier that both lost all their hosts on 2026-10-26 ~08:25 PDT (their deployments were removed). Every conversation turn failed with "no hosts available" from service discovery. After the language model was fixed, a 05:53 call connected but the agent heard nothing: the owner's audio reached the backend and speech recognition failed the same way, so every transcript was empty and nothing answered. Read-aloud takes the backend's direct verbatim-read path and never calls the language model, so a read-aloud probe cannot see this class of failure. The backend's speech-error event (added 2026-10-26, #546 read-aloud-silent-audio) reported only the asyncio TaskGroup wrapper, not the exception inside it, and called it a synthesis failure.

Done 2026-10-27 (private deployment; vibe-talk unchanged):
- Backend pointed at the production language-model tier for the same model (05:36 PDT). Conversation verified through the relay at 05:38: transcript plus 830 ms of speech.
- Backend pointed at the production speech-recognition tier (05:56 PDT). Spoken input verified through the relay at 06:01: the agent transcribed "What is 2 plus 2?" and answered aloud "Two plus two equals four." The other development tiers it uses were checked: one more is empty but not used by this deployment.
- A private smoke test that opens a real ticketed session through vibe-talk's authenticated relay (#553 voice-ws-relay-auth), as the page does: GET /api/v1/voice-session, open the single-use /api/v1/voice-socket URL, require session_started, send a typed prompt, require an assistant transcript and audible speech (10 ms RMS > 500) before turn_complete, quit; an audio-input probe that speaks a question into the session the way the page's microphone does (audio_start, the greeting, then PCM at the session's input rate, with the server's turn detection ending the turn) and requires a user transcript and a spoken answer; plus a verbatim read. Error frames fail the probe verbatim, with the first real exception from the backend log appended.
- The owner's adaptive cadence, as a user timer plus a state file: hourly until 6 passes in a row, then every 2 h for 12 more hours of passes, then every 4 h until Friday 2026-10-30 23:59 PDT; any failure resets to hourly. Every failed run notifies the coordinator with the error text; the first pass after a failure notifies once.
- Backend draft change: the speech-error message unwraps exception groups and names the first leaf exception; an authorization denial inside a group still gets the access-denied code. Unit tests pass. A release with it is staged and starts cleanly on test ports; not yet deployed.

# Acceptance Criteria

A conversation failure on the production voice path is detected within the current cadence interval (1-4 h) and reported to the coordinator with the underlying error, not only the read-aloud path. Open item: deploy the backend build whose error message names the sub-exception (waits for a maintenance window; the owner is using voice), then check that a forced failure's error frame names the leaf exception.
