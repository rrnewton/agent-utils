---
title: 'vibe-talk-theme-settings-pane: a theme settings pane with separate light and dark configs and an extra-dark default'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T06:35:00-05:00
updated_at: 2026-11-23T06:35:00-05:00
---

# Description

Owner request for full theme customization: separate Light and Dark configurations (with manual or system switching), each either Default or Custom; an Extra dark default alongside the legacy dark mode, substantially darker and tuned; and custom themes that start from the default and let the reader pick every color, grouped into a moderate number of categories.

# Design

- Research first (a short report): background, card and text values of the dark modes of comparable apps, to tune Extra dark.
- Audit current CSS color usage and propose the color groups (neither too coarse nor too fine), for example a second highlight for the Discuss mobile title bar besides the blue accent.
- The settings pane toggles between editing the light and the dark configuration; each is Default or Custom; custom starts from the default.
- The owner reviews the proposal (groups plus Extra dark values with shots) before the full build.

# Acceptance Criteria

- The proposal (research report, color groups, Extra dark values with shots) is reviewed by the owner before implementation.
- After the build: page tests for switching and persistence; shots of the pane and of each default theme.

# Notes

Owner UI request (fast-track); filed by the coordinator.
