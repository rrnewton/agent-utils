---
title: 'vibe-talk-screenshots-discuss-config-scenes: run.sh''s Discuss-config scene list misses 93-98, 102 and 103'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- screenshots
created_at: 2026-11-25T08:41:00-05:00
updated_at: 2026-11-25T08:41:00-05:00
---

# Description

Found while evidencing #2231 vibe-talk-selection-strip-atomic. `scripts/run.sh --screenshots` gives the throwaway server a serving message cache and a `[summaries.llm]` section (which is also what turns on WTF? glossary lookups) only when an `--only` state matches a hand-written case list (73-76, 78, 84, 85, 89, 90, 91, 92, 99). The glossary and strip scenes 93-glossary-orphan through 98-glossary-orphan-rerender, 102-chip-row-rigid and 103-chip-row-below all start from the same glossary button act as 92, yet are missing from the list. Run alone (for example `--only 97-glossary-orphan-popover`), the walk stops before any shot: "the Discuss window stayed empty after the server's history walk". They only work when some listed state rides along in the same run.

# Design

- One source of truth: the scene table in `scripts/screenshots.py` says which scenes need the Discuss config (a scene field, or derived from the acts that need glossary lookups or Discuss), and `run.sh` asks it (for example a `--needs-discuss` query of the selected states) instead of keeping its own list.
- Until then, at least add 93-98, 102 and 103 to the list.

# Acceptance Criteria

- `scripts/run.sh --screenshots --only 97-glossary-orphan-popover` (and each of 93-98, 102, 103 alone) runs and photographs the state.
- The screenshots self-test has a control that fails when a scene needing the Discuss config is missing from what `run.sh` configures.

# Notes

Filed at the coordinator's request as a side finding of #2231; worked around there by running 97 beside 92.
