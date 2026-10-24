---
title: 'read-aloud-pieces: split long and bulleted blocks into pieces for the read-aloud controller'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-24T10:44:14.509581889+00:00
updated_at: 2026-10-24T13:02:35.799617741+00:00
closed_at: 2026-10-24T13:02:35.799616990+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792793510, 2026-10-23, phone screenshot of a coordinator message whose bullets are typed '•' lines in one paragraph): "I see that this message has bullets in it why is it not split up into chunks with the floating controller for me to navigate within it? Shouldn't we have a Max chunk size and words? And even if a message is one giant paragraph, shouldn't we still split it even if we split it evenly in words. Obviously splitting along paragraph or list structures is much preferable to picking arbitrary breaks inside a paragraph. Having some heuristic to at least identify sentence boundaries would be nice." Today a read is cut only at drawn elements (#376 read-aloud-blocks), so a '•' list typed as text, or one long paragraph, is one block.

# Acceptance Criteria

A drawn block is read in pieces by one rule on both sides (server source, page drawn text): a bullet-glyph line is its own piece, lines between are one piece, a piece over a word maximum is cut at sentence ends, and only one over-long sentence is cut evenly by words. The controller counts and steps pieces; the element is lit and the sentence light stays within the piece. Rust and page tests share one fixture; a Chromium walk shows a '•' message read as several pieces.
