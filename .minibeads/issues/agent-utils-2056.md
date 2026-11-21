---
title: 'vibe-talk-reader-retry-and-error-telemetry: retry recoverable read-aloud failures and count them'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- voice
- read-aloud
created_at: 2026-11-21T10:46:00-05:00
updated_at: 2026-11-21T10:46:00-05:00
---

# Description

[watermelon-full-preview] Owner request 282 (2026-11-21): the voice reader died mid-bullet-item within a message. Restarting it continued the read, but the perceived behaviour was that it just died and stopped reading, with no reason given. The owner asks for a retry loop: retry recoverable errors, log recoverable errors, and keep telemetry that clearly shows the error rates so the ones under our control can be fixed later.

Prior art: #487 (a `lost` part retries three times at 1s/3s/8s on fresh tickets from the last heard segment) and #1872 (one retry at the same segment for `no_audio`/`malformed`) cover failures the page's player itself observes. Neither covers a read whose audio fetch goes away without the player observing a failure, and neither is counted anywhere: there are no read-aloud error-rate counters.

## Evidence (2026-11-21 incident, deployed server's `app.log` plus code)

One read (261 chars, block 2 of its message) reached audible playback at 15:25:06Z and its server turn ended at 15:25:16Z as `interrupted=true superseded=true`, with no warn or error anywhere. The owner's restart replayed the same block at 15:25:58Z; it ended the same way at 15:26:08Z. Two more turns of the same message died the same way by 15:27:18Z.

What the code and log together prove:

- A `superseded=true` turn end (`speech.rs`, `SessionTask::complete`) means the server sent the provider an Interrupt. Only two paths do that: a new selection superseding the turn (`accept`), and the turn's HTTP consumer going away (`abandon_current`, when the audio response's event channel closes).
- Not a supersede: no new audio request reached the server at any of the death times, and after each interrupt acknowledgement the pending queue was empty (nothing was prompted next; the next turn each time was the owner's restart, tens of seconds later). A superseding request prompts its successor at once.
- Not the voice stack: the provider session (generation 1) opened at 15:20:56Z and served every turn of the incident, reused throughout; it did not end until 15:32:52Z, for an unrelated interrupt-acknowledgement timeout. No session end, socket error or provider error stands near the deaths.
- So the server interrupted each turn because the page's audio response went away about ten seconds into playback, every time. The page reported nothing: the whole day holds zero `the page stopped a read-aloud` warnings and zero `audio stream ended in an error` warnings, and the page sent no failure query and made no recovery fetch after any of the deaths. It went silent until a reload-shaped re-entry (client-settings, stream and timeline fetches) just before each restart.
- Today the two interrupt causes log identically, and a superseded stream error is deliberately not recorded (`http/api.rs`: a read superseded by another selection returns before the failure is kept), so this class of death is invisible server-side by construction. On the page, a player that aborts its own fetch (`silence()` in the streamed player) or a page that stops running reports nothing either. Which page-side trigger fired (the player's own abort through some stop path, or the page/connection itself going away) cannot be told from today's telemetry; that is part of what this issue fixes.

# Design

1. A bounded automatic retry for recoverable read-aloud failures: resume at the same block (no re-reading already-heard text where avoidable), a small fixed number of attempts with backoff, and the user told only if it gives up (a short visible notice). Never retry a deliberate stop or a user-initiated supersede.
2. Structured logging for every read-aloud failure, server- and page-side, page failures reported to the server through the existing failure endpoint or a new small one: class, recoverable yes/no, retried, retry outcome. The server's interrupt log learns to say which cause fired (superseded by a new selection, or the reader went away), so the next incident is diagnosable from the log alone.
3. Per-day error-rate counters, queryable from `healthz` or a stats endpoint: failures by class, retries attempted, retries recovered, give-ups.
4. The existing #487/#1872 retries keep their counts and waits; this issue extends the same treatment to the failure classes that currently end a read silently, and counts all of them.

# Acceptance Criteria

- A read-aloud failure that is recoverable (a dropped or cut audio stream, a fetch that goes away mid-block, a provider turn that fails after audio where a fresh attempt can succeed) resumes at the same block after a backoff, up to a small fixed number of attempts, without re-reading blocks already heard.
- After the last attempt the read stops with a short visible notice; a deliberate stop and a user-initiated supersede are never retried and never show that notice.
- Every read-aloud failure, page- or server-observed, lands in the server log structured (class, recoverable, retried, outcome), and per-day counters by class are readable from `healthz` or a stats endpoint.
- Fixture tests only (a fake TTS sidecar or stream that drops mid-block; never the live page or voice stack): the retry resumes at the same block, gives up after the fixed count with the notice, the counters increment, and a deliberate stop is never retried.
- Screenshots of the give-up notice for coordinator review (phone and desktop, light and dark) before deploy.

# Notes

Cause reported to the coordinator before any fix, per the lane brief; implementation waits for the go. Related: #487 read-aloud-recovery, #548 cloud-read-failures, #1872 vibe-talk-read-no-audio-retry, #2057 vibe-talk-slack-rate-limit-check (the same log, a separate question).

[watermelon-full-preview] Coordinator GO (2026-11-21) on the scope as filed, with two additions: (1) server-side, distinguish the two interrupt causes in the log and in the counters (`abandoned` vs `superseded`); (2) page-side, every player stop path records why (user stop, arrow or jump, new selection, error, visibility or connection lost), non-user reasons are reported to the server, and the bounded same-block retry fires only for recoverable non-user reasons. Also checked against #2055 (a live server panic on the speech-text path, `speakable.rs` `internal_id()` splitting at byte 1 of a segment starting with `…`): the log holds 15 such panics on 2026-11-21 (12:23-12:46Z and 15:29-15:48Z clusters), and none falls within two minutes of any of the four 15:24-15:27Z deaths — the panic explains other failed speech work that day (a panicked prepare drops its request; the page sees a lost/refused read, never a supersede, and a block whose text panics never reaches a turn), while the 15:25Z deaths remain the separate page-consumer cause above. The panic fix itself is #2055, landed separately.
