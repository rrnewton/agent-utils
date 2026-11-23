---
title: 'vibe-talk-dark-true-black-cards: in dark mode, message cards get a true black background'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T06:35:00-05:00
updated_at: 2026-11-23T10:37:42-05:00
closed_at: 2026-11-23T10:37:42-05:00
---

# Description

Owner, using the page in dim light: compared with apps that have true black backgrounds, our dark mode is mostly grey. First experiment: message cards, which fill most of the screen, get a true black background.

# Design

- Dark theme only: the message card background becomes #000, with text, borders and dividers adjusted only as needed to keep contrast and card separation readable.
- Light theme unchanged. A small change shipped first; the full theme work is #2155.

# Acceptance Criteria

- Shot matrix: phone and desktop, dark (cards true black) and light (unchanged), Discuss open.

# Notes

Owner UI request (fast-track); filed by the coordinator.

## Resolution

[watermelon-full-preview] Shipped and deployed. In dark only, message cards (agent, owner, third-party) use a true black background (#000000) in both the read and unread states; summarised cards keep only their tint bar and edge, never a tinted fill. Speakers stay distinguishable by edge colour, side inset, corner and tile geometry, and light is unchanged. Landed on main as ea8d7479 (gated tree 1f9995d7, full gate green); the coordinator confirmed deployment with a clean watch.

Evidence: screenshot matrix (both themes, phone and desktop, Discuss open, keyboard open and closed, plus summarised and done-feedback states) with dark card centres sampled at (0, 0, 0) and the light frame unchanged; the three colour-contract browser checks now assert the distinction where it lives in dark (edge and geometry, tint over black) and keep their light fill checks.
