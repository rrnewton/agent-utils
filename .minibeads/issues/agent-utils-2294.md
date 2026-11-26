---
title: 'vibe-talk-dark-composer-black: in dark mode the message box gets the same black as the message cards'
status: open
priority: 1
issue_type: feature
assignee: cl-composerblack
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T10:20:00-05:00
updated_at: 2026-11-26T10:20:00-05:00
---

# Description

Owner request 385 (2026-11-26 10:13 AM ET), verbatim: "(Btw, I would like the black background to apply to the COMPOSE window too because I look at that a lot.)"

Since `#2154 vibe-talk-dark-true-black-cards` the message cards in dark are true black, but the message box (composer) where the owner types keeps its grey dark background.

# Design

- Dark theme only; light unchanged. The composer's background reuses the token the dark message cards use, never a new hard-coded colour, so the `#2155 vibe-talk-theme-settings-pane` editor can still override it.
- Cover the channel composer, and check every other text box the owner sees as a compose window (thread reply, side and reply composers, the Discuss "Ask about this" field); list which changed and which did not, and why.
- Text, placeholder, caret and border stay readable on black (contrast ratios stated in the resolution).
- The page backdrop is not touched (that is `#2197 vibe-talk-dark-pane-background-darker`).

# Acceptance Criteria

- In dark, the composer's computed background equals the message card's computed background; in light it is unchanged. A page check asserts both at phone and desktop sizes.
- Screenshot matrix (light and dark, phone 412 and desktop 1280, keyboard open and closed, Discuss open) plus before and after of the dark composer, each folder with BUILD.txt.
- Contrast of text, placeholder and border on the new background is reported.

# Notes

Owner-UI fast-track. Owning lane: cl-composerblack. Related: `#2154 vibe-talk-dark-true-black-cards` (the card black), `#2155 vibe-talk-theme-settings-pane`, `#2197 vibe-talk-dark-pane-background-darker` (the backdrop, separate).
