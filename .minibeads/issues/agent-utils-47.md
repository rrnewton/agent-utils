---
title: 'read-aloud-overlong: a full read stalls about 75 s mid-response and runs far past its spoken length'
status: in_progress
priority: 2
issue_type: bug
assignee: opus-5.5/read-aloud-overlong
labels:
- vibe-talk
- audio
depends_on:
  agent-utils-37: discovered-from
created_at: 2026-09-26T09:35:28.961358042+00:00
updated_at: 2026-09-26T09:35:28.963597195+00:00
claimed_at: 2026-09-26T09:35:28.963597195+00:00
claimed_until: 2026-09-28T09:35:28.963455011+00:00
---

# Description

[opus 5.5] Split out of #37 read-aloud-cut-truncation, which closed on its acceptance. That issue was about the wrong message, or only a few words, being read after a cut. This one is about full reads that run far too long.

Symptom: a full read of a message of about 1,000 characters produces 140-180 s of audio. The same message read after a cut takes about 68 s. The listener hears the read drag on.

Evidence, content-free, from the deployed internal provider bridge's logs on 2026-09-25 and 2026-09-26:
- In each overlong response, the voice provider keeps streaming audio frames at real-time pace. Meanwhile no transcript text arrives for 61-77 s, and most often for 75-76 s. Then text resumes and the read finishes.
  - The total transcript stays at about the message length (989-1,055 characters), so the message is not repeated.
  - The response then ends normally. It is not a turn that never ends.
- The stall often begins 3-4 s into the response. Its length is nearly constant, which points to a timer upstream of the bridge. The provider's decoder kept a steady output rate through the stall.
- It predates the #37 read-aloud-cut-truncation fix:
  - It hit 9 of 29 responses of 30 s or more on at least four earlier builds.
  - Those reads ended at 119.4 s, about halfway through the message. The bridge's former 120 s whole-response limit cut them off without reporting it.
- Since that limit became an idle timeout, it has hit 5 of 8 such responses, and they now run to their natural end: 145 s, 165 s, and 179 s interrupted.
- Not yet known:
  - Whether the audio during the stall is silence or speech that has no transcript.
  - Whether the rate is the same on a build without the fix.

Why this repository cares: a read that stalls for over a minute looks broken. The page keeps a completed read's message archived only once the read ends.

Owner: opus-5.5, the agent that delivered the #37 read-aloud-cut-truncation fix. Plan:
1. From the provider's per-response token and audio-chunk timing, decide whether the text stream stalls or the speech synthesis emits filler. Measure audio energy during the stall.
2. If the cause is in the bridge, fix it there with a regression test.
3. If the cause is in the provider, hand it the timings. This issue stays open until the acceptance test passes live.

# Acceptance Criteria

Run live on the deployed stack and report content-free:
- 10 fresh-session full reads of long messages, about 1,000 characters each.
- No response has more than 15 s where audio flows but no transcript arrives.
- Every read ends with a completed response.
- Audio seconds per transcript character stays at or below 0.09. The post-cut reads measured 0.068 and the stalled control 0.157.
- The #37 read-aloud-cut-truncation re-answer probe still reports 0 of 2.
