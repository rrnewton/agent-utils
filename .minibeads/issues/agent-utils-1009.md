---
title: 'agentctl-collapsed-paste-taken: a prompt Claude collapses to a pasted-text placeholder is reported not seen'
status: open
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T11:20:00.000000000+00:00
updated_at: 2026-11-07T11:20:00.000000000+00:00
---

# Description

When agentctl pastes a long, multi-line prompt (for example a failed-delivery digest) into a Claude Code pane, Claude can collapse it in the composer, and in the transcript row it takes into its turn, to a placeholder such as "[Pasted text #3 +12 lines]". The placeholder carries none of the prompt's words, so the read-back that #953 and #989 improved cannot match it. The delivery is then recorded as not-seen, or as a failed digest, although the prompt reached the agent. #989's resolution notes this gap.

# Design

- After a send, treat a pasted-text placeholder as the prompt taken. It must appear where the prompt should be (above or in the composer, after the send), and its line count must match the pasted text's (+M lines, computed the same way Claude counts). Accept it only when no other paste happened in that window.
- Record the evidence on the message (placeholder text, its #N, the line count), the same way #953 records a queued receipt.
- A placeholder whose line count does not match, one that was already on screen before the send, or one with a different #N sequence than expected stays unproven, as today.
- Rust edition first; say in the change whether the Python edition reads placeholders at all.

# Acceptance Criteria

- A test: a multi-line prompt that Claude collapses to "[Pasted text #N +M lines]" with the matching M is recorded as delivered once, with the placeholder as evidence.
- Negative controls: a placeholder with the wrong line count, one already present before the send, and a placeholder from a different paste each stay unproven.
- No change to how a prompt with visible words is matched.

# Notes

Requested 2026-11-07 by the lead coordinator. Follows #953 (queued receipt counts as delivered) and #989 (own drafts submitted, digests matched by content).
