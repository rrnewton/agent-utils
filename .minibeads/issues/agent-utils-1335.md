---
title: 'bang-answer-visible: bang answers typed at top level must not hide in a thread'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-11T18:05:00+00:00
updated_at: 2026-11-11T18:30:00+00:00
closed_at: 2026-11-11T18:30:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 396: `!who` in coord7's channel looked like it did nothing. The bridge did answer (deterministic bang post), but `send_pending_bang_posts` posts every bang answer as a thread reply under the source message's thread, so a bang typed at top level hides as a thread reply under the owner's message.

# Design

- Cancelled before implementation landed; see Notes.

# Acceptance Criteria

- None: cancelled by the owner.

# Notes

## Resolution

Cancelled by the owner (reply 400): "No, I disagree with that -- answering in a thread is a good default almost alwasy." Bang answers stay as thread replies (current behaviour). The gated local commit was never landed or pushed; no revert is needed.
