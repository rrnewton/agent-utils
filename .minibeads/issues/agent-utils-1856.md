---
title: 'vibe-talk-read-aloud-follow-not-yanked-up: read-aloud follow is not yanked up by the drill keeper'
status: open
priority: 0
issue_type: bug
assignee: mu-readerdrill
labels:
- vibe-talk
- ui
created_at: 2026-11-18T12:48:00-05:00
updated_at: 2026-11-18T12:48:00-05:00
---

# Description

[watermelon-full-preview] Owner report, urgent (owner thread 191, 2026-11-18), verbatim: "I was in the process of listening to a long message in read aloud mode and it kept jumping up higher. In the scroll back. I expected it to keep the part it's reading right on the screen, the sentences that were towards the last 25% of a long message, but instead it just kept jumping back up to the start of the message where the sentences that were being read were off the screen. If I scroll it myself, then I guess it's a design question. How aggressive it should be in re-centering the red text while it progresses reading from chunk to chunk? ... But if I don't scroll at all myself then by default I would expect this to certainly stay where it was at or at. Not randomly scroll upward. And in this case, if I'm reading along message I do actually want it to automatically scroll down and keep the reading on screen. Maybe the exception to that could be if you scroll away entirely such that no part of the message being read is on the screen then that can be interpreted as an intentional user action that wants to keep the reading off screen to look at something else. ... if you do break away from the tracking mode which is automatically following the reading then it should have a chip that jumps back to the message being read clearly visible along the bottom of the screen with the other hovering chips, just like the bookmark one."

Root cause (liaison trace, thread 191): read-aloud's `followReading` writes scrollTop DOWN to keep the read sentence on screen, then the #1818 drill keeper (`placeDrillAnchor` via `payThreadEntry`, re-fired by its Resize/Mutation observers on every thread repaint) writes it back UP to the drilled message's top — 7 times in 3 minutes, up to −570 px, with no user scroll. Two scroll owners with no priority between them. #1839a stood the keeper down for a focused composer, but not for an active Read.

# Design

Express "Read is following" as a priority in the existing scroll-keeping owner, not a new writer:

- While Read is actively following (playing, not held, cursor in view), the drill keeper and the message-anchor holders stand down, as they already do for a focused composer (#1839a): no re-placement may move scrollTop up over the reading cursor. With no user scroll, the read sentence stays on screen and follows DOWN through the message.
- A user scroll that still leaves part of the read message on screen pauses following as today (`followPause`); following resumes at the next sentence boundary only when the cursor is about to leave the viewport (the measured proposal, stated in the landing report).
- A user scroll that takes the read message fully off screen leaves follow mode and shows a "back to reading" chip in the bottom floating-chip row (like the bookmark chip); tapping it re-enters follow mode at the cursor.

Coordinates with #1839b (rows fix) and #1846 (one anchoring authority), which touch the same holders; lands right after #1838.

# Acceptance Criteria

- A deterministic browser test (condition waits only) through the real built page, with Read running in a thread view and live repaints arriving, asserts scrollTop never moves up while following, and the read sentence stays on screen.
- The same walk covers: user scroll fully off the read message -> follow mode leaves and the back-to-reading chip shows in the bottom chip row; tapping it re-enters follow at the cursor.
- Dynamic evidence plus screenshots (phone and desktop, light and dark, keyboard closed) are captured on a test instance and reviewed.
- #1818 anchor behaviour with no Read running is unchanged (thread-drill-anchor walk still passes).

# Notes

Owner-priority gate: deployed on landing. Lane mu-readerdrill on branch mu-readerdrill/1856, worktree readerfollow. Lands right after #1838 vibe-talk-reader-continues-into-thread.
