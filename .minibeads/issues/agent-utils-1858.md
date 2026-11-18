---
title: 'vibe-talk-followup-find-cites-asker-message: a find-the-message follow-up links the askers own message'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-18T13:30:00-05:00
updated_at: 2026-11-18T13:30:00-05:00
---

# Description

[watermelon-full-preview] Liaison live check of the #1844 fix (owner thread 180's exact question, main scope): the route answers 200 with clickable citations, but a "find the message where I ask about X" question re-summarizes agent replies and never links the owner's own message that asks about X. That message can sit after the saved summary's through_id, outside the window the follow-up context is built from, so the model never sees it. The follow-up should find and link the specific message first — the asker's own message for "where I ask" — and answer short instead of re-summarizing.

# Design

- The follow-up's context fetch (still cache-only) spans from the saved summary's window through the newest held message, bounded: when the summary's through_id is not inside the fetched window, step the walk back until it is or the bound is reached, owner's own messages included like any other.
- The follow-up prompt carries a standing find rule: a question asking to find a specific message answers short, names and cites that message first (the owner's own [OWNER] message for "where I ask"), and does not re-summarize the window.
- No change to the first summary's window, its fetch, or its prompt.

# Acceptance Criteria

- An end-to-end test through the real router seeds a summary, then a later owner message asking about X after the summary's through_id, asks "find the message where I ask about X", and shows the model was sent that message and the find rule, and the answer cites the asker's message id (only the model backend stubbed).
- A follow-up whose summary window already covers the asker's message behaves as before apart from the find rule.
- The context extension is bounded and cache-only: no provider call is added (the chat fake's read count does not move).

# Notes

Owner-priority fast-track from the #1844 live check (liaison, 2026-11-18). Test instances only; no live tests. #1848 vibe-talk-route-health-alert continues after this.
