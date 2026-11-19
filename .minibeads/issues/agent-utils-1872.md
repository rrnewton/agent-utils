---
title: 'vibe-talk-read-no-audio-retry: one retry for a cloud read part with no speech or a malformed body'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T16:30:00-05:00
updated_at: 2026-11-19T01:15:00-05:00
closed_at: 2026-11-19T01:15:00-05:00
---

# Description

[watermelon-full-preview] Filed from #1866's landing investigation (coordb's question, 2026-11-18). The page's streamed read gives up immediately, with no retry, on two part failures: `no_audio` (no speech heard within `READ_NO_SPEECH_MS` = 4000 ms of a started part's first audio, `guardSpeech` in `vibe-talk/web/src/view/voice/read-aloud.ts`) and `malformed` (a body that stops inside a frame, or audio the decoder cannot read, in the streamed player's reader loop). By contrast a `lost` part (fetch TypeError, 404/410, cut-off stream) is retried three times at 1 s / 3 s / 8 s on fresh tickets from the last heard segment (`#487`, `READ_RECOVERY_WAITS_MS`).

There is no timeout on a slow-but-progressing stream: the body reader has no deadline and neither does the fetch, so a busy TTS backend that keeps answering is waited out. The eager cases are a part whose first audio arrives but whose speech does not follow within 4 s (vendor pre-roll, scheduling starvation), and a truncated/undecodable body (an aborted prefetch can produce one). A live user on a slow network or a busy backend could see the read stop the same way.

# Design

Give `no_audio` and `malformed` one retry at the same segment, on fresh tickets like a `lost` retry (forget prepared speech first), before the read gives up. Keep the counts and waits of `#487` as they are; the retry is once, not a new schedule. A second failure of either kind gives up as today, with the same wording.

# Acceptance Criteria

- A streamed read whose part first arrives with no speech, or with a malformed body, is tried once more at that segment and continues when the second try speaks.
- A part that fails the same way twice stops with today's wording and archives nothing.
- Existing read-aloud suites (cloud failure, recovery, silence) pass unchanged apart from the new retry.

# Notes

Related: #487 read-aloud-recovery, #548 cloud-read-failures, #1866 vibe-talk-read-follow-clears-reader-strip (the investigation that measured the thresholds).

## Resolution

[watermelon-full-preview] Landed on github/main as 9ed5771e2fd7aa36830e551bfde3e99f2a032549 (gated head 97dab254, owner gate validate: OK). A streamed part that arrives with no speech (`no_audio`, 4s from its first audio — the threshold already counts from first audio, so it was kept rather than raised) or truncated/undecodable (`malformed`) is tried once more at the same segment on fresh tickets, by the jump path; a second failure gives up with the previous wording. Manual dynamic evidence (handoff): with a fake whose block 3 body was truncated once, the block was fetched twice (205:3 x2) and the read completed 8 of 8; a 4.5s-silence-lead body did not stop the read. The #548 and part-way page tests now assert the retry-then-give-up contract.
