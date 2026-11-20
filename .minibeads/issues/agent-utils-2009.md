---
title: 'vibe-talk-windowed-viewport-fill: windowed window must fill the viewport after a fold'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- scroll
created_at: 2026-11-20T17:59:00-05:00
updated_at: 2026-11-20T17:59:00-05:00
---

# Description

[watermelon-full-preview] Owner-visible on the deployed page (found in the coordinator's post-landing windowed screenshot walk, 2026-11-20): at desktop with windowed lists ON, after pressing ';' ("Long messages: all expanded"), the anchored card is correct but the entire viewport below it is one empty spacer band - no rows. The OFF twin and the phone render rows. Same class as #1985's windowed All-view fixes.

# Design

- Cause: the window is sliced in estimated-height space. After a fold changes real row heights everywhere, the priced slice covers far fewer real rows than the viewport needs, so the mounted window ends above the visible band's foot and the remaining list stands as the bottom spacer below it.
- After mounting, if the mounted rows do not reach the visible band's foot (and more groups stand below as spacer), shift the window down to fill the band, and symmetrically upward when it starts below the band's head. Direction-agnostic, so over- and under-estimating prices both fill the band, and re-toggling "all expanded" on and off cannot strand the reader on spacer.
- The test asserts, after the fold settles, that the point three quarters down the visible band is a message row - at desktop and phone, windowed ON, with "all expanded".

# Acceptance Criteria

- No settled empty viewport band after a fold at desktop or phone with windowed lists ON, toggling "all expanded" on and off.
- Dynamic evidence in real Chromium (windowed All walk with the new band assertion, refresh walk, send walks), gate on the final SHA, then land on the coordinator's LAND NOW.

# Notes

[watermelon-full-preview] Number pre-assigned by the lead coordinator (2026-11-20). Related: #1985 vibe-talk-windowed-all-view-fixes (landed as b1c721cc; its coverage shift only fired when less than one row of the band was covered, which is why this case survived), #2008 vibe-talk-windowed-send-frame-drop.
