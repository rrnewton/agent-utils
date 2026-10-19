---
title: 'column-overscroll: let the right column scroll past its end so the newest card can sit mid-screen'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T19:03:00.285839259+00:00
updated_at: 2026-10-19T10:29:31.876748108+00:00
closed_at: 2026-10-19T10:29:31.876747998+00:00
---

# Description

[opus 5.5] Owner, 2026-10-15: on a desktop monitor, the newest thread in the right column sits at the very bottom of the screen, which is where the reader's attention is but the least comfortable place to read. Keep that as the default landing position, but let the mouse wheel (and keyboard paging) scroll PAST the end into blank space, up to the point where the last card's top sits at half the column's height, so the reader can bring the card they are reading to the middle. Applies to the right column's thread list and thread view on desktop layouts (not the phone, unless it reads naturally there too). Scrolling back down returns to the normal foot; 'at the newest' and the jump control treat the overscroll region as still at the foot; #280 scroll-stability rules still hold (nothing moves under a reader who did not move). Chromium checks at 1280x800 and 1920x1080.
