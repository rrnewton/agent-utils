---
title: 'vibe-talk-saved-chip-covers-header-label: the Saved status chip covers the channel header label'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T08:00:00-05:00
updated_at: 2026-11-24T08:00:00-05:00
---

# Description

In the main view (phone), the transient "Saved HH:MM - refreshing..." status chip overlays and covers the channel header label ("N MESSAGES FROM ..." seam line), so the header cannot be read while the chip is up. Seen in the #2155 Part A evidence shot theme-main-dark-custom.png (pre-existing; not caused by the theme editor).

# Design

- Keep the chip from covering the header label: offset the chip below the header seam, move the label, or reserve room while the chip is shown; the chip stays transient and dismissible as today.

# Acceptance Criteria

- Shot matrix (phone and desktop, light and dark): with the Saved chip up, the channel header label is fully readable; chip behaviour otherwise unchanged.

# Notes

Filed by mu-themes at coordb's direction during #2155 liaison review; fix after #2155 A2 unless trivial.
