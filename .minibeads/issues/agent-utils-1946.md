---
title: 'agentctl-codex-resume-paused-goal-picker: recognise Codex Resume paused goal picker in prompts/answer, never auto-answer'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- codex
- goals
created_at: 2026-11-20T05:10:00-05:00
updated_at: 2026-11-20T05:10:00-05:00
---

# Description

[watermelon-full-preview] Codex's "Resume paused goal?" picker
blocked a revived lane's queue for 2.4 hours. The operator chose
"Leave paused" by hand, then `agentctl drain` delivered 3 queued
prompts. As with the Replace-goal picker (#1939), `prompts` could
not name the picker and `answer` could not choose in it.

# Design

- Recognise the picker with the same strict rules as #1939
  agentctl-codex-replace-goal-picker (exact recorded text, last
  block on screen, footer ending the block, re-read before keys),
  report it in `prompts`, and let `answer` choose in it.
- Never auto-answer it from any delivery, revive, or restore flow.
  Open question for the implementer, to justify in the change:
  whether revive may answer "Leave paused" when the revive itself
  caused the prompt (leaving a goal paused is the conservative,
  state-preserving choice; resuming work the owner paused is not).

# Acceptance Criteria

- `agentctl prompts NAME` on the recorded picker screen returns
  the question with its options and selection, and
  `agentctl answer NAME --option N` answers it with a re-read
  before each key.
- No agentctl flow auto-answers the picker, except, if justified
  and implemented, revive answering "Leave paused" only for a
  picker its own revive caused.
- Tests use recorded pane text; the harness support matrix Codex
  rows match the code.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1946, queued after
#1939 lands, coordinated by coordb. The exact recorded picker text
must be captured from a live Codex pane or test fixture before the
strict matcher is written; do not guess its wording.
