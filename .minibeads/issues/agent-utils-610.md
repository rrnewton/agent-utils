---
title: 'mobile-keyboard-compose: iOS hides the message box; flush scroll must not cap later scrolling'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-28T04:03:27.328833450+00:00
updated_at: 2026-10-28T04:03:27.328833450+00:00
---

# Description

[opus 5.5] Follow-up to closed #575 mobile-keyboard-compose. The owner, 2026-10-27, on a real iPhone: the jump that puts the message box flush on the keyboard's top is perfect on Android, but on iOS it scrolls so that the ENTIRE message box is hidden, and the list then will not scroll further to recover it or to show Send ("We made it a hard maximum"). Wanted: jump to the flush position once as the keyboard opens, but never constrain the reader's scrolling afterwards.

Wanted:
1. On iOS the box's foot at visualViewport.offsetTop + visualViewport.height (the top of everything iOS draws over the page: keys, QuickType bar and the floating form accessory control), as on Android; Android left as it is.
2. The flush position is a jump, once per keyboard open, focus move or keyboard size change, never a cap: no scroll or pan listener pulls a reader back.
3. Say whether a web page can hide iOS's form accessory control (Done, previous/next).
4. An opt-in on-screen readout of the numbers the page sees (innerHeight, visual viewport height/offsetTop/pageTop, the box, the target) so a screenshot from a real iPhone pins down any remaining mismatch. Off by default.
5. Tests for the iOS model (visual viewport shrinks and pans, the layout does not) and the Android model (the layout resizes): the box lands flush and later scrolling is not capped.
