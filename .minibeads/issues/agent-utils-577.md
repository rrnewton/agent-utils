---
title: 'read-chunk-balance: a paragraph read aloud is cut into balanced pieces, and the timeline into balanced widths'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-27T13:07:51.684213992+00:00
updated_at: 2026-10-27T13:08:03.217002322+00:00
closed_at: 2026-10-27T13:08:03.217002222+00:00
---

# Description

[opus 5.5] The owner, 2026-10-27 05:06 (condensed): a medium-length single paragraph of several sentences was read as 2 chunks, one huge (all but the last sentence) and one tiny (the last sentence); the scrubber showed one wide block and one small one. Wanted:
1. An explicit balance metric in the block splitter (words per block plus skew). Bullets and other structure may stay skewed, but a paragraph is split by sentences into balanced blocks; this split should have failed the metric.
2. For a single message, prefer sentence-level blocks for a short paragraph (several balanced blocks filling the scrubber), even if the same paragraph would be one block inside a bigger document.
3. Scrubber block widths should not let one block take most of the width.
4. Check whether this case is a bug, fix it, and add the owner's example as a test; a small step toward the multi-level scrubber if it falls out.

Cause: read_pieces packed sentences greedily up to READ_PIECE_WORDS (60). The owner's paragraph was 61 words (sentences of 5, 18, 30, 8), so it was cut 53 + 8.

# Acceptance Criteria

The owner's paragraph cuts 23 + 38 on server and page (shared fixture); the skew metric fails 53/8; a one-paragraph message of short sentences is several balanced steps; timeline widths compressed and capped; before/after screenshots; the gate green.
