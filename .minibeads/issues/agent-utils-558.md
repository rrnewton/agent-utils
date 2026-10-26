---
title: 'pack-triangle-clear-bridge: the pack triangle''s rectangle must not touch the thread bridge''s line'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-26T21:40:17.721941906+00:00
updated_at: 2026-10-27T00:24:36.590145150+00:00
closed_at: 2026-10-27T00:24:36.590145029+00:00
---

# Description

[opus 5.5] Follow-up to #526 pack-triangle-dim (286c9802). Owner, 2026-10-26 14:06: "I'm glad the upward pointing triangles are a bit dimmed.. they moved a bit up but i don't think they moved LEFT like I asked. They are still almost touching the vertical bridge. I want them to move left enough that the rounded rectangle button that comes up when I hover is not intersecting the bridge. Visual overlap between objects like that looks sloppy and bad."

Measured before (tests/bridge_even_browser.py): the button box overlapped the bridge line by 9.9px at 768 and 1280 wide, and by 4.3px on phones (393, 412). Wanted: the full button box at least ~3px clear of the line at 393x852, 412x915, 768x1024 and 1280x800 in both schemes, the triangle still centred in it, its tip just below the card above. A phone's gutter left of the line is 28.3px, narrower than the 32px target plus the room: a trade-off for the owner there.
