---
title: 'scrub-bar-no-play-pause: the floating read-aloud strip keeps Previous and Next, not Pause or Stop'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-26T15:25:07.017132189+00:00
updated_at: 2026-10-26T15:30:25.063020748+00:00
closed_at: 2026-10-26T15:30:25.063020628+00:00
---

# Description

[opus 5.5] The owner (2026-10-26): "let's remove the pause/play from the floating scrub bar. I like how we currently have stop and pause on the lower bar. We don't need to redundantly have it. We can just have the next and previous in the floating controls."

# Acceptance Criteria

The floating strip (#read-steer) holds its grip, Previous, the position, Next and the block timeline; no Pause/Play and no Stop. The lower bar's Read (Pause/Resume) and Stop, the media keys and every keyboard shortcut unchanged. Page tests and the Chromium walk updated; Help says it.
