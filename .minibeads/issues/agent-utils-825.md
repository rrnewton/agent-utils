---
title: 'agentctl-stranded-multi-paste: attribute composers holding several agentctl pastes or paste plus delivered text'
status: closed
priority: 1
issue_type: bug
assignee: mu-restore2@devbig030
labels:
- agentctl
created_at: 2026-11-03T18:04:46.642771266+00:00
updated_at: 2026-11-03T18:55:53.400143171+00:00
closed_at: 2026-11-03T18:55:53.400143051+00:00
---

# Description

[opus 5.5] Follow-up to #808 agentctl-stranded-draft-blocks-delivery: with #808 deployed (watcher, auto-drain router and agent-teams on 2e5ca138), a working Claude Code coordinator's deliveries were blocked twice more on 2026-11-03 by composer contents #808 does not attribute:
1. 12:20-12:58 ET: two concatenated placeholders, "[Pasted text #231 +1 lines][Pasted text #232 +4 lines]" (about 14 agentctl prompts and 3 agent-teams messages parked).
2. ~13:01 ET: a placeholder followed by typed text, "[Pasted text #233 +4 lines][from cl-refactor via agentctl ...]" (the auto-drain stderr log, new in #808, showed the refusal).
Each was cleared by one Enter in the pane. #808 attributes only a single exact placeholder matching the latest delivery.

# Design

- Attribute a composer that consists only of agentctl's own recent deliveries: any sequence of placeholders whose labels match recorded deliveries, and literal text that matches the start of a recorded delivery's prompt, in any order. Submit it (one Enter) and continue draining.
- Find out why agentctl's typed prompts are left unsubmitted in a working Claude Code composer at all (the Enter after a paste not registering mid-turn?) and fix that root cause if it is in agentctl.
- Anything containing text agentctl did not type stays untouched and is a watcher finding, as #808 already does.

# Acceptance Criteria

- Tests: two stranded placeholders; a placeholder followed by the literal start of another delivery; both are submitted and the queue drains.
- A composer containing human-typed text is still left alone and reported.
- The root cause of the unsubmitted paste is written in the issue, fixed if it is agentctl's.
