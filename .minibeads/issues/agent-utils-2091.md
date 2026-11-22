---
title: 'vibe-talk-shrink-anchor-policy: archiving or folding a message in fold-read mode must keep the reader oriented'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T05:50:00-05:00
updated_at: 2026-11-22T05:50:00-05:00
---

# Description

The owner: "I seem to experience a number of scenarios or archiving a message makes the screen jump when we're in the fold-read mode. This is a scenario where it's important for me to stay oriented because I'm trying to move to the next unread message and read it. I articulated this principle before where if the region above the thing disappearing is visible. I want it to stay put and the region below should move up to it. If I'm archiving something that's disappearing which goes off the top of the screen, then in that case the line that stays still should be the one that's still on the screen at the bottom of the thing disappearing. If the thing disappearing is so huge that it goes off the bottom and the top of the screen, the best we can do is try to make the center of the screen exactly at the seam between the prefix before The disappeared object and the suffix after it which now come together. Of course, if archiving message introduces a new fold line that itself takes a little space, so it's more like a collapse of a bigger object to a smaller object."

# Design

One shrink and remove anchor rule for archive (Done) in fold-read mode and for any collapse of a message to a fold line: (1) if the item's top is on screen, the content above holds still and the content below moves up; (2) if the top is off-screen above and the bottom is on screen, the content just below the item holds still and the content above moves; (3) if it spans both edges, the seam (where the content before and after meet, at the fold line) lands at the viewport's centre. It composes with the grow rule of #2076 in the same scroll owner, and with earlier archive anchoring work (#543, #1514, #1889, #1975, #2034, #2038).

# Acceptance Criteria

- Measured anchor deltas of 0 px (cases 1 and 2) and the seam at the centre (case 3), windowed on and off, phone and desktop; deterministic frame-based browser checks through the real router; marked before and after shots of one action each.

# Notes

Filed by the coordinator; the owning lane refines this file as it works.
