---
title: 'read-aloud-blocks: read a long message block by block, showing which block is playing and letting the reader move between blocks'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T17:03:15.063483579+00:00
updated_at: 2026-10-19T19:05:31.544010761+00:00
closed_at: 2026-10-19T19:05:31.544010610+00:00
---

# Description

[opus 5.5] Owner request 2026-10-19: he listens to long messages in the car, and a long message (paragraphs, bullets) is read as one block with no way to see or change where he is in it. Split each message into the blocks it is drawn as (paragraphs, list items, block quotes, headings; a code block or a table is one short spoken summary), speak them in order without gaps at the boundaries, highlight the block being read with 'k of n', let a tap on any block jump there, and add Previous and Next controls plus Media Session handlers so steering-wheel and Bluetooth buttons move between blocks. Later steps, filed separately: sentences inside the current block, and a block timeline scrubber.

# Acceptance Criteria

A multi-block message plays every block in order, with the boundary gap measured near zero; the playing block is highlighted with 'k of n'; tapping a block jumps there; Previous, Next and the Media Session previoustrack, nexttrack, play and pause act on blocks. A one-block message, the voice call, read new and the device voice behave as before. Page tests with negative controls, and a Chromium walk of a multi-block message against a fake voice backend asserting highlight, 'k of n', tap-to-jump, Previous/Next, the media-session handlers and the gap timing.
