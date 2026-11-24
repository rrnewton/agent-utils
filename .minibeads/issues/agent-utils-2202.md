---
title: 'vibe-talk-screenshots-glossary-button-scene-order: scene 92 must skip glossary-linked text scene 91 learned'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-24T06:45:00-05:00
updated_at: 2026-11-24T06:45:00-05:00
---

# Description

Screenshot scenes share one server per run. Scene 91-glossary-wtf learns the term "flaky ordering test" (laptop/dark combo), after which it renders as a .glossary-term button. Scene 92-glossary-button (_act_glossary_button) hardcodes that term and does not skip .glossary-term nodes, so its drag selects linked button text, no quotable selection stands, and the scene times out whenever it runs after scene 91. _act_glossary_wtf already guards this (per-combo terms, skipping linked nodes); _act_glossary_button needs the same guard. Reproduced on an untouched tree (91 then 92 in sequence fails; each passes alone).

# Acceptance Criteria

- _act_glossary_button skips .glossary-term nodes when finding selectable text (and tolerates the selection being cleared by linking, like _act_glossary_wtf).
- Scenes 91 and 92 in sequence pass 3 times in a row.
- Test-only change; no page code changes.

# Notes

Found by the mu-wtflayout lane while evidencing #2187 vibe-talk-wtf-chips-one-row-clear-of-handles / #2195 vibe-talk-wtf-chip-orphan-lifecycle.
