---
title: 'vibe-talk-read-follow-clears-reader-strip: read follow keeps the lit sentence clear of the reader strip'
status: closed
priority: 0
issue_type: bug
assignee: mu-readerdrill
labels:
- vibe-talk
- ui
created_at: 2026-11-18T16:25:00-05:00
updated_at: 2026-11-19T01:10:00-05:00
closed_at: 2026-11-19T01:10:00-05:00
---

# Description

[watermelon-full-preview] Found by coordb reviewing the #1856 shots (owner thread 191 follow-on, 2026-11-18): on DESKTOP the floating reader strip (Auto, the arrows, "7 of 8", React, the progress cells) and the reading-mode notice cover the lit sentence — `1280x800-2-thread-following.png` in the #1856 set shows the lit sentence at the strip's top edge and partly under it, and `1280x800-4-back-to-reading.png` shows it behind the strip. Phone is fine. The follow's band is measured from the scroll area alone, so a sentence the follow has correctly kept "on screen" can still stand under the strip that floats over the list's foot.

# Design

In the follow (`followReading` in `vibe-talk/web/src/view/voice/read-aloud.ts`), treat the reader strip (`#read-steer`) and the reading notice as covering the viewport bottom: measure their rects live at each follow (the strip can be dragged, the notice comes and goes) and end the follow band above whichever of them stands over the list. The lit sentence is then kept above them, on phone and desktop alike. No other holder changes.

# Acceptance Criteria

- A browser assertion at 1280x800 and 412x915 that the lit sentence's rect never intersects the strip's rect while reading, through several sentences, including after a live arrival.
- Shots on the #1855 screenshot harness (real server) of the thread-following and back-to-reading states, phone and desktop, light and dark, with no failure notice.
- Path-selected owner-priority gate.

# Notes

Owner-priority gate: deployed on landing. Lane mu-readerdrill on branch mu-readerdrill/1866, worktree readerstrip. Follow-up to #1856 vibe-talk-read-aloud-follow-not-yanked-up.

## Resolution

[watermelon-full-preview] Landed on github/main as 6665a5070ef1aa251928a5c3a8a207fec4cef673 (fix replayed as ea5b0b08; gate pass on 0a803fada2a94611d9e40f6796f55208f5d7ed56). The follow band ends above the reader strip and the reading notice, measured live at each follow. Shots on the #1855 harness are in /tmp/mu-readerfollow-shots/harness (device voice: the mock vendor serves only the conversation endpoint, so no sentence is lit there; lit-sentence evidence is the follow walk, whose strip-intersection assertion is green at both sizes, base intersecting at 1280x800).
