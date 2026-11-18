---
title: 'vibe-talk-dark-bar-icon-invisible: the thread picker glyph vanishes under a forced theme'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- regression
created_at: 2026-11-18T13:57:05-05:00
updated_at: 2026-11-18T13:57:05-05:00
---

# Description

[watermelon-full-preview] Regression from #1845 vibe-talk-appearance-setting (cfbfe817, live). On every dark phone shot the bottom bar's thread-picker button (between the channel picker and the sparkle) is blank. The Appearance control set the picker's inline `background-image: none` for a forced Dark choice, erasing the glyph; and a forced Light choice on a dark device left the dark-scheme (pale) glyph on the light button. The glyph is a scheme-keyed SVG data URI in `web/styles/voice/18-control-bar.css` (`#thread-select`): pale stroke by default, dark stroke under the light media block, so any forced theme that disagrees with the device scheme picks the wrong one.

# Design

- Remove the inline `background-image` override from the Appearance control; it never owned the glyph.
- Give the forced themes their own glyph in the same phone block: `:root[data-theme="dark"] #thread-select` takes the pale stroke, `:root[data-theme="light"] #thread-select` the dark stroke, each beating the device media block by specificity. System keeps the media behaviour untouched.

# Acceptance Criteria

- On a test instance, in Dark forced, System with a dark device, and Light forced with a dark device (and the mirror cases), the picker's computed `background-image` glyph stroke contrasts with its button background; no bar or Settings icon is blank in either forced theme.
- A browser test asserts that contrast per theme case, and the screenshot sweep of the bar in each case is inspected.

# Notes

Small separate change, path-selected owner-priority gate; coordb deploys at once when green.

## Resolution

Fixed: the inline override is gone and each forced theme carries its own glyph in
18-control-bar.css. Verified in Chromium (tests/appearance_icon_browser.py, 6 cases):
glyph contrast 13.43:1 (dark) and 16.90:1 (light) against the button, and the bar
sweep inspected with every other icon visible.
