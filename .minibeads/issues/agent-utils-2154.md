---
title: 'vibe-talk-dark-true-black-cards: in dark mode, message cards get a true black background'
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

Owner, using the page in dim light: compared with apps that have true black backgrounds, our dark mode is mostly grey. First experiment: message cards, which fill most of the screen, get a true black background.

# Design

- Dark theme only: the message card background becomes #000, with text, borders and dividers adjusted only as needed to keep contrast and card separation readable.
- Light theme unchanged. A small change shipped first; the full theme work is #2155.

# Acceptance Criteria

- Shot matrix: phone and desktop, dark (cards true black) and light (unchanged), Discuss open.

# Notes

Owner UI request (fast-track); filed by the coordinator.
