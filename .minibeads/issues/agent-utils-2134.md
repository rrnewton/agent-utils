---
title: 'vibe-talk-discuss-timeline-expand-inline: the Timeline expand control sits straight right of its header'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T17:35:00-05:00
updated_at: 2026-11-22T21:05:49-05:00
closed_at: 2026-11-22T21:05:49-05:00
---

# Description

The Timeline header expand control sits to the right and below the header, wasting vertical space.

# Design

Place the expand control inline on the header row, immediately right of the Timeline label, with no extra row.

# Acceptance Criteria

- Header and control share one row on phone and desktop; no vertical space added; screenshots light and dark, Discuss open.

# Notes

Owner UI request (fast-track); filed by the coordinator.

## Resolution

[opus 5] Landed 9707e35f on github/main: the Timeline toggle stands in #discuss-timeline-head, straight right of the label on one row (phone header block 71.22px -> 44.03px, desktop 63.22px -> 36.03px, measured). Gate 0f0024b5 red only on vibe-talk.test.page-3 CPU-TIMEOUT; coordb waiver on local page-3 638/638 and verified combined tree. AFTER shots from landed main in /tmp/mu-discussexpand-shots-landed.
