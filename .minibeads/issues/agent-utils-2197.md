---
title: 'vibe-talk-dark-pane-background-darker: darken the dark backdrop outside the black message cards'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T06:25:00-05:00
updated_at: 2026-11-24T06:25:00-05:00
---

# Description

Owner 341 (2026-11-24): after reloading with the true-black message
backgrounds (#2154 vibe-talk-dark-true-black-cards), darken the background
OUTSIDE the messages somewhat — still a contrast with the pitch black of
the message backgrounds, but not as much of a contrast as now.

# Design

- Dark theme only; light unchanged. Message cards stay `#000000`
  (`--mine`/`--theirs`/`--third`).
- The outside surfaces are tokens, never hard-coded colours, so the
  #2155 theme editor can override them: `--bg` (page floor, timeline
  backdrop, space between cards; survey group G1), with `--panel`
  (pane background, Discuss pane) and `--dock` (bars and dock, G5)
  darkened in step so the outside hierarchy (cards darkest, then
  backdrop, then pane) is preserved. No new backdrop token.
- Consistent with the #2155 survey's Extra dark (`--bg #000000`): this
  change moves Standard dark toward black without reaching it; Extra
  dark remains the separate true-black tier. Candidates are shot and
  the liaison picks before landing.

# Acceptance Criteria

- Candidate backdrop values shot on phone 412 and desktop 1280, dark,
  with read and unread cards, Discuss open, keyboard closed and open;
  folders with BUILD.txt sent to the coordinator before landing.
- Only the picked candidate lands, as token values in the dark tables
  (sheets, THEME_TOKENS, pre-paint script, theme-color), with the
  default Discuss header following `--panel`.
- Focused colour-contract checks pass; screenshot matrix re-shot from
  landed main.

# Notes

Owner UI request (fast-track, owner rule 286). Prior work: #2154
vibe-talk-dark-true-black-cards (cards `#000`), #2155
vibe-talk-theme-settings-pane (survey Extra dark values).

[watermelon-full-preview] mu-darkbg
