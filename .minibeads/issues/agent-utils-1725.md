---
title: 'vibe-talk-undo-chip-stretched: the Undo chip stretches down the right edge with the keyboard up and the Discuss half-pane open'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T17:03:39.655035304+00:00
updated_at: 2026-11-16T17:03:39.655035304+00:00
---

# Description

Owner report 2026-11-16 11:57 AM ET (fast-track, owner rule 286), Android phone in the light theme, on-screen keyboard open in the thread reply composer with the Discuss half-pane docked open: the Undo chip renders as a tall white pill down the right edge of the pane, covering message text, the right of the reply textarea and the Send reply button. The owner: "I assume it was not intentional that this undo button have a very strange tall shape along the right margin of the pane. It looks like we tried to move it out of the way and stretched it instead". Root cause (cascade): the keyboard-open relocation from #1680 (b391ff27) moves #scroll-tools to the top of the list with bottom:auto at specificity (2,3,0), but the docked Discuss half-pane rule from #1526 (3044dac0, vibe-talk/web/styles/voice/22-discuss.css) sets bottom at (3,2,0) and wins, so top and bottom both apply, the strip stretches between them, and the lone visible Undo chip stretches with it. Sibling lanes own the other two defects in the same screenshot (#1724 vibe-talk-discuss-theme, #1726 empty red banner); do not touch those areas. The fix must keep the chip normal-sized, moved clear of the composer, never stretched, and never over Send or the textarea, in light and dark, keyboard open and closed.

# Acceptance Criteria

Page tests at phone 412x915 (device scale 2.6) and desktop 1280x800, in light and dark theme, with the keyboard emulated (visualViewport shrink) open and closed and the Discuss half-pane open in the phone shots: the Undo chip keeps a normal chip height, does not intersect the reply textarea or the Send reply button, and the chips strip is not stretched; condition-based waits only (owner rule 346). Screenshot matrix per the standing owner-UI acceptance (owner thread 124, 2026-11-16) on a local test instance with fixture data: light and dark x keyboard open and closed x phone and desktop, Discuss half-pane open in at least the phone shots, plus the targeted state before and after; every image looked at by the lane and reviewed by the liaison before announcement.
