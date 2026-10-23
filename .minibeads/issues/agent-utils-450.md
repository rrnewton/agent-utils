---
title: 'pack-triangle-hue: a plain, larger pack triangle in its thread''s colour'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-23T13:06:33.030627982+00:00
updated_at: 2026-10-23T14:41:34.769000483+00:00
closed_at: 2026-10-23T14:41:34.769000123+00:00
---

# Description

[muse watermelon] From #440 thread-card-pack-toggle. The owner: "I notice the little up-arrow collapse icon is wrapped in a circle. I just want a triangle, though bigger than the current triangle. It also does not match the thread color and is always blue. I want it to match the color."

Wanted: a plain, larger up-triangle with no circle or chip background (a generous invisible hit target is fine and desirable, at least 32px on touch), drawn in the thread's own hue (the server-assigned hue from #427, the same colour as the thread's left edge and bridge), including hover/focus/active states derived from that hue, in light and dark schemes and under Unpack unread (where read cards' edges go grey, the triangle follows the edge). Keep its aria-label, tooltip and the ^ key.

# Acceptance Criteria

Page tests: no disc or chip behind the triangle; the triangle is larger than the old one in its disc; the hit target is at least 32px; its colour matches the card's left edge and the bridge in light and dark; under Unpack unread a read thread's triangle is grey as its edge is; hover/focus/active are derived from the same colour; aria-label, tooltip and ^ still pack. Chromium screenshots at 393x852 and desktop, both schemes.
