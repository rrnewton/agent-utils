---
title: 'vibe-talk-overlays-center-on-channel-pane: floating chips and toasts center on the channel pane when Discuss is docked'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T17:00:00-05:00
updated_at: 2026-11-17T17:00:00-05:00
---

# Description

[watermelon-full-preview] Floating chips and toasts over the channel (the undo chip, the "Ready." toast, the "Newest (n)" pill, and any other floating overlay over the channel) are centered on the whole screen rather than the channel pane when the Discuss panel is docked, so the undo chip crosses into the Discuss region. They must center on and stay within the channel pane, on desktop side-by-side and on phone half-pane. There should be one shared positioning helper.

# Design

- Find every floating overlay over the channel and route its horizontal centering through one shared positioning helper anchored to the channel pane.
- Scope: overlay positioning only.

# Acceptance Criteria

- With Discuss docked, each floating chip/toast bounding box lies inside the channel pane at desktop (1280x800) and phone (412x915) sizes.
- Page tests cover the docked case at both sizes; the screenshot matrix is captured and reviewed.

# Notes
