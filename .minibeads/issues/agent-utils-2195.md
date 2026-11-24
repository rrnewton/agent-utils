---
title: 'vibe-talk-wtf-chip-orphan-lifecycle: the WTF? chip is never left standing after the selection chip group hides'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T05:45:00-05:00
updated_at: 2026-11-24T05:45:00-05:00
---

# Description

Owner 340 (2026-11-24): the WTF? chip sometimes stays on screen after Quoted reply and the dismiss X are gone, floating mid-paragraph with no selection. WTF?, Quoted reply and x must appear and hide together on every path: selection cleared, scroll, tap elsewhere, popover close, and re-render. Folded into #2187 vibe-talk-wtf-chips-one-row-clear-of-handles (same root cause: separate placement/visibility in the glossary view vs the quoting view).

# Acceptance Criteria

- One chip-group show/hide path: no path leaves WTF? visible while the rest of the group is hidden.
- A page test for each dismiss path asserts no orphan chip remains.
- Orphan-check screenshot (state after dismiss) in phone dark and light.

# Notes

Owner UI request (fast-track, owner rule 286). Prior work: #2156 vibe-talk-wtf-glossary-lookup, #2187 vibe-talk-wtf-chips-one-row-clear-of-handles.
