---
title: 'documents-desktop-control-bar: group the reader controls into one centred Reader box on desktop'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T18:30:00.000000000+00:00
updated_at: 2026-11-14T18:30:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI request 066 item 8: on desktop the Documents control bar scatters its reading controls. Group Reader + cloud/device audio + speed/Stop/Pause into one centred "[Reader ...]" box. Outline and Fetch go on the right; they never show together (Fetch = list view, Outline = reader view). Popups (outline, speed, voice, etc.) centre over their button and are clamped on-screen. Phone layout unchanged unless it shares the popup positioning code (then clamp there too).

# Design

- On desktop (the existing `min-width: 900px` and `pointer: fine` regime) the control pane, while it holds the reader's controls, is one bordered box named "Reader" centred in the dock, holding the device/cloud audio toggle, Pace, Stop and Read.
- Outline and Fetch are pinned at the bar's right end beside the hamburger; Fetch is hidden while a document is open and Outline only stands in the reader, so the two never show together.
- A shared popup placement (`centrePopupOver`) centres the outline, Fetch menu, Fetch-since prompt and pace popups over their buttons and clamps them inside the viewport, on desktop and phone alike.
- The phone tile keeps its layout; only the shared popup clamp reaches it.

# Acceptance Criteria

- At desktop widths the reader controls stand in one centred, bordered box labelled "Reader" containing the audio toggle, Pace, Stop and Read.
- Fetch shows in the Documents list view and Outline in the reader view, at the bar's right; they never show together.
- The outline, Fetch and pace popups open centred over their buttons and fully on-screen at desktop and phone widths.
- The phone control bar and tile are otherwise unchanged.

# Notes

Owner UI request 066 item 8 (2026-11-14). Sibling lanes on the same page: #1540, #1541, #1542, #1543, #1545.
