---
title: 'pace-stepper: the Pace popover is a [-] [100%] [+] stepper, with no slider'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- read-aloud
- ui
created_at: 2026-10-26T23:00:02.158082267+00:00
updated_at: 2026-10-26T23:00:02.158082267+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26 14:14: "When I pop open Pace the slider is actually SUPER annoying. In fact let's completely get rid of it. Let's have a text box in the center for the percent, initially storing "100%". Then buttons flanking it for (-) and (+). Popping it open and clicking plus a few times is what I want." Remove the slider. In its place [-] [100%] [+]: 10% steps within 50-200% (the server's clamp, 0.5x-2.0x, which the device voice's rate follows); the box takes a typed value with or without % on Enter or blur; arrow up/down step it; buttons at least 44px on a phone. Stored and synced under the same key as before, and applied to both voices at once, mid-read included.

# Acceptance Criteria

Page tests for stepping, clamping, typed values, arrows and a mid-read change by either voice; a Chromium walk at 393x852 touch and 1280x800 with screenshots.
