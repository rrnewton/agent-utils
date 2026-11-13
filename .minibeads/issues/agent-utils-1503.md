---
title: 'react-dictation-keeps-listening: React dictation keeps listening until stopped or sent'
status: closed
priority: 1
issue_type: bug
assignee: mu-reactmic
labels:
- vibe-talk
created_at: 2026-11-13T23:19:00.000000000+00:00
updated_at: 2026-11-13T23:19:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request 047 (11-13 6:19 PM ET), verbatim: "Also, a problem that I've noticed from the start was when I paused the reader and I press react to react to the currently highlighted sentence. It is able to enable the microphone but it's not in the normal way where my keyboard is open and the little microphone icon is highlighted. It's in some other way where the microphone has been directly turned on and it transcribes for about a sentence, but it very quickly times out and then will no longer listed to me or transcribe. Why do we have a limit on how long this microphone mode listens and why is it separate from the Android keyboard microphone?"

React from the paused reader starts the page's own dictation (`vibe-talk/web/src/view/composer/dictation.ts`, `#821`/`#829`), not the keyboard microphone. It sets `continuous = false`, and `onend` finishes with nothing restarting it. Chrome on Android ends a recognition session at the first pause, so it listens for about one sentence and stops for good. `#829` deliberately keeps the keyboard down (`inputmode=none`).

# Design

Build (A) only; (B) is pending the owner's answer (see Notes):

- While the React box is open in listening mode, keep listening until the owner stops it or sends: `continuous = true` and restart on `onend` / `no-speech` / network blip, as long as the box is still open and listening.
- Never restart after an explicit stop, after send, after the box closes, or on a permission error (`not-allowed` / `service-not-allowed`).
- Keep the settled (final) text across restarts so nothing is lost or duplicated; interim text is replaced, not appended.
- Show a clear mic-on state on the box with tap-to-stop (the existing `#quick-reply-mic` badge and `data-listening`), and a visible stopped state when listening really ends.
- Cap runaway restarts on the event count, not a timer (owner rule 346): after N consecutive empty sessions, stop and show "tap to listen again".
- Structure the start path so (B) — a setting "React uses: page microphone / keyboard microphone" — can be added later without reworking the restart logic.

# Acceptance Criteria

- Module tests with a fake `SpeechRecognition`: end restarts while open; no restart after stop/send/close/permission error; final text is kept across restarts without duplicates; interim text is replaced; the empty-session cap stops and says to tap to listen again. No wall-clock waits.
- The existing dictation browser check is extended for the new end behaviour where there is one.
- Selected local checks pass, then the owner-priority gate passes, then the change lands on main.

# Notes

Owner UI fast-track (owner rule 286), request 047. Lane mu-reactmic from coordb.

(B) "React uses: page microphone / keyboard microphone" is NOT built here; it is pending the owner's answer. The code is structured so it can be added later.

## Resolution

[watermelon-full-preview] Built (A). `createDictation` (`vibe-talk/web/src/view/composer/dictation.ts`) now hears with `continuous = true` and restarts a fresh session whenever one ends while the box is still open and listening, riding out `no-speech` and network ends the same way. Settled words are kept across restarts exactly once and unsure words are replaced, never appended. An explicit stop (the microphone badge), a send and the box closing all stop through the same `stop()`, and a permission error (`not-allowed`, `service-not-allowed`, `audio-capture`) ends listening with the keyboard-microphone line; none of those restart. Listening shows the existing mic-on badge with tap-to-stop, and a real stop says "Stopped listening. Tap to listen again." on the box. Empty sessions are counted, never timed: after `DICTATION_EMPTY_SESSION_LIMIT` (5, banded 2..20 in the page ground-rules) in a row, listening stops with that stopped line. (B) remains pending above; `start()` is the seam where its choice would go.

Tests: the `#1503` module test in `vibe-talk/tests/js/view/composer.test.mjs` (fake recognition: restart while open, no restart after stop/permission, settled-once and unsure-replaced across restarts, the empty-session cap, speech resetting the count), the updated `#821`/`#829` module tests, and the React walk in `vibe-talk/tests/read_pause_reply_browser.py` (a restarted session still listening with its words kept once, and a stopped state on the open box). The selected local validation passed, including that browser walk and the page ground-rules.
