---
title: 'vibe-talk-overlays-center-on-channel-pane: floating chips and toasts center on the channel pane when Discuss is docked'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T17:00:00-05:00
updated_at: 2026-11-18T04:53:23-05:00
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

## Resolution

[watermelon-full-preview] Landed on main: fddd3b80 (the shared helper `--channel-pane-end` / `--channel-pane-bottom` in 22-discuss.css, fed by the root `--discuss-width` / `--discuss-docked-height` from panel.ts; the chips row, pull report, status pill and read-aloud strip consume it; the keyboard corner Send stands at the pane's foot), 77fe78ef (follow-ups: the status pill waits while the whole server-search section stands at the foot; the floating Discuss sheet stands clear of the composer's row; the Summarize hours field shows five digits), and 0dd3ae64 (the screenshot clear-state fixture sends a valid channel). Covered by tests/channel_pane_overlays_browser.py at 412x915 and 1280x800 in light and dark, plus the Discuss walks; the screenshot matrix is in /tmp/mu-chipcenter-shots (before/after) and /tmp/chipcenter-shots-1813b. One gate step (bridge-gutter) passed only on its exclusive rerun and solo 3/3 on both trees; it is untouched by this change.
