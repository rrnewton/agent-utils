---
title: 'voice-page-read-nav-center: centre the N of M indicator in one row and keep React always visible'
status: open
priority: 0
issue_type: bug
assignee: cl-refactor@devbig030
labels:
- vibe-talk
created_at: 2026-11-03T22:00:33.282856901+00:00
updated_at: 2026-11-03T22:00:33.282856901+00:00
---

# Description

[opus 5.5] OWNER P0 (2026-11-03 16:11-16:15 ET), feedback on #821 as deployed. The owner sees the larger Reading-mode arrows, but the cluster is pushed right and too close to the edge, and the "N of M" indicator is no longer centred; React and dictation work on their Android phone. "I want it in the same row. I don't want to add another row."

# Design

- Centering: everything on ONE line. The "N of M" indicator sits at the exact horizontal centre of the row; the back/forward arrows are symmetric around it with a comfortable margin from the screen edges; React comes after the arrows in the same row; an invisible balancing spacer at the left end, as wide as React, is ALWAYS reserved. Never wrap: on narrow phones shrink the gaps and the count before anything wraps.
- React is ALWAYS visible, not only when paused. Pressing it while playing pauses the read and opens the quoted reply on the CURRENT sentence immediately (same as the paused path).
- Style: React is a rounded rectangle in the row's orange palette (the strip's colours and tokens; no blue).
- Centering may land alone first if React-always-visible would add more than about 30 minutes.

# Acceptance Criteria

- Browser test: the row is one line, and the indicator centre equals the row centre within 2 px at 360, 375, 412 and 1280 px, with React shown.
- React is visible while playing; pressing it while playing pauses and quotes the lit sentence.
