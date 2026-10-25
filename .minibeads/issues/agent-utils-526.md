---
title: 'pack-triangle-dim: the pack triangle dimmed, up and left, centred in its hover rectangle'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-25T21:41:50.416798579+00:00
updated_at: 2026-10-25T23:26:35.890389218+00:00
closed_at: 2026-10-25T23:26:35.890388948+00:00
---

# Description

[muse watermelon] Owner, 2026-10-25: "let's dim those up arrows... The up arrows for collapsing the unpacked thread children look kinda bad. I want to keep them the same hue but DIM them some, and move them a bit up and left. They should be close to touching the bottom of the message. When I hover them they appear as a colored rounded rectangle. The arrow/triangle should be horizontally centered in that rectangle." The pack triangle (#450 pack-triangle-hue, repositioned by #501 thread-bridge-gutter): same hue at lower strength at rest, full strength on hover/focus/press; moved up and left so its tip nearly touches the bottom edge of the message (card) above; the triangle horizontally centred in the rounded-rectangle highlight its button draws on hover, and vertically sensible in it. Its tap target stays at least 32px (36px on the phone as now) and the phone left-edge placement from #501 stays consistent; light and dark themes both checked.

# Acceptance Criteria

At 393x852, 412x915 and 1280x800 in dark and light: the triangle rests visibly dimmer than its hover/focus strength in the same hue; its tip sits within a few px below the bottom edge of the card above; the triangle's horizontal centre matches its button rectangle's within a pixel or two at rest (the rectangle the hover background fills); its target is at least 32px each way and on the phone its ink stays a few px from the screen edge (#501). Packing still works by tap and click. Page tests assert the centring and the gap; screenshots in the Chromium walk.
