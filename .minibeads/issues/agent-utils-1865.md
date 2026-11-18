---
title: 'vibe-talk-followup-find-answers-rest: a find answer also answers the rest, in one line'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-18T15:50:00-05:00
updated_at: 2026-11-18T15:50:00-05:00
---

# Description

[watermelon-full-preview] Liaison live check 2 of the #1858 find behaviour: the find itself passes — the owner's message is cited first, quoted, short, no re-summary. Two refinements remain. The owner's question had a second part ("What's the binary TCB?") that the find-only answer never addresses. And the widened context grew the follow-up's input from ~60k to ~146k chars (5.5s to 27s live), because the walk steps back to the whole recap bound.

# Design

- Prompt: after the citation line, the answer addresses any remaining part of the question in ONE short line, citing the message it comes from.
- Bound the widening: the follow-up fetch steps back a bounded number of gap messages past the summary's through message (not the recap bound), and gap messages ride in the context trimmed to a per-message cap. The asker's message stays included: the walk still steps back from the newest through the gap, and the first fetch's window plus the summary always ride whole.
- The first summary's fetch and prompt are unchanged, as is every other follow-up whose summary window already reaches the present.

# Acceptance Criteria

- A router test with the owner's two-part question asserts the prompt's find rule requires the one-line answer to the rest, citing its source.
- A router test with a long gap (hundreds of held messages between the summary window and the newest, the asker's message inside the bounded reach) asserts the asker's message is in the context sent to the model, the answer cites it, and the context size stays under a stated cap.
- Cache-only is unchanged: the chat fake's read count does not move across a follow-up.

# Notes

Owner-priority, from the #1858 live check (liaison, 2026-11-18). Runs in parallel with #1848 vibe-talk-route-health-alert. Test instances only.
