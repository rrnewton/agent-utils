---
title: 'vibe-talk-discuss-followup-prompt-and-labels: follow-up answers restate owner messages and look like the question'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T17:50:00-05:00
updated_at: 2026-11-22T17:50:00-05:00
---

# Description

In the Discuss timeline, a follow-up question is followed by an answer line that reads like a recap of an earlier owner message, so it looks as if the question was rewritten. Code audit: the question is stored and drawn verbatim; the answer is the model output. Follow-ups reuse the summary briefing system prompt (effective_system_prompt with no override in the follow-up handler), which invites the model to cite and restate owner-tagged channel lines. The page shows question and answer as two paragraphs that differ only by CSS, with no label, and the folded header shows only a time.

# Design

- A follow-up-specific system prompt: answer the question directly, never restate the question or an owner message as the answer, no workstream sections, keep citations.
- Page: show the verbatim question in the folded follow-up header, and label each paragraph (You / Discuss) so model text can never sit in the question place.

# Acceptance Criteria

- Server test through the real router: a follow-up request uses the follow-up prompt, not the briefing prompt (stub only the model upstream).
- Page test and screenshots (phone and desktop, light and dark, Discuss open) showing the labelled question and answer and the question in the folded header.

# Notes

Owner UI request (fast-track); filed by the coordinator from the thread 323 code audit.
