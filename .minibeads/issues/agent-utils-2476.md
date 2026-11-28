---
title: 'thread-reader-resume-timer-speech: thread_reader_resume_browser fails "the reader did not resume on Back" on heads that never touched vibe-talk, because its speech stub ends utterances on a 12 ms-a-character timer; known-red until 2026-12-24, then a condition-driven stub'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
- hot-host
created_at: 2026-11-28T12:10:00-05:00
updated_at: 2026-11-28T12:10:00-05:00
---

# Description

`vibe-talk.check.thread-reader-resume-browser` (`vibe-talk/tests/thread_reader_resume_browser.py`) was red on 11 gate heads in the 7 days to 2026-11-28, 3 of them in the last 48 hours. 6 of the 11 heads never touched vibe-talk, including a wrkslots-only head on 2026-11-28 at 7:05 AM ET. The kept step output shows two assertions:

- `1280x800: the reader did not resume on Back`: the walk waits for the next reply to be said again in All after Back.
- `1280x800: the thread resumed on 'Reader drill chunk one opens the long deployment note for the lane.', not the chunk being read '...'`: the walk's mid-chunk Thread tap.

An earlier red ran 134 s wall for 18.5 s of CPU, so the step was waiting, not working.

The walk's waits are already conditions with the hang backstop (#2077). What races is the stub, `DEVICE_VOICE`. Each utterance ends on `setTimeout(..., 12 * text.length)`, about 0.7 s for a reply part. On a busy host the walk's tap, its waits and its click can take longer than that, so the utterance the walk means to interrupt has already ended and the read has moved on. The resume then names a different utterance than the one the walk asserts. This is a wall-clock race in the test (owner rule 346), not a product failure the gate can see.

# Design

- **Interim (liaison, 2026-11-28):** the step is known-red until 2026-12-24 on the coordinator's list. If gate.py should waive it as well, add a `validation/known-red.toml` entry naming this issue with `expires = "2026-12-24"`.
- **Fix:** a condition-driven speech stub. An utterance starts synchronously when spoken, as now (#2051), and ends only when the walk ends it, through a page hook such as `window.deviceSpeechEnd(index)`, or when it is cancelled. No timer. The walk then:
  - ends chunk one;
  - taps Thread while chunk two is speaking;
  - asserts the resume;
  - ends the thread's anchor utterance so the read goes on to the next reply;
  - clicks Back while that reply is speaking, and asserts it is said again in All.

  Every step waits on a condition, bounded only by the hang backstop.
- Check the other walks that use the 12 ms-a-character stub (`#540`'s) for the same race, and list them in Notes when fixing.

# Acceptance Criteria

- The speech stub in `thread_reader_resume_browser.py` has no timer; utterances end only when the walk ends them or the page cancels them.
- The walk passes 20 of 20 runs with host load1 at or above 237 (75% of 316 hardware threads), recorded with the load per run.
- The known-red entry is removed when this lands, before 2026-12-24.

# Notes

Filed by `cl-redaudit` (number pre-assigned by the coordinator, 2026-11-28), from the frequent-red gate audit of 2026-11-28 (decision approved by the liaison).

Related:
- #2077: hot-host browser waits.
- #2175: the interim browser known-red entries, which expire 2026-12-24.
- #2051: synchronous speech onset.
