---
title: 'voice-page-modularity-plan: two independent refactor proposals for the voice page, then one judged final plan'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-28T12:47:41.385672322+00:00
updated_at: 2026-10-28T15:29:48.343694601+00:00
closed_at: 2026-10-28T15:29:48.343694451+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 08:42 ET: Opus 5.5 (cl-refplan) and Muse (mu-refplan) each read vibe-talk/web/voice.js (61,039 lines, one file) and independently propose clean abstractions and modules. A third agent (the owner asked for Astra, gpt-6-astra, currently access-restricted) compares both, picks, improves, and writes the final plan under ai_docs/transient/. Owner direction: TypeScript modules with a build step and minified output; a portable non-DOM core (API/sync, state, search language, speech logic, read marks, bookmarks, settings model) so a later React Native port can reuse it. Proposals: ~/agent_scratch/voice-refactor/plan-opus.md and plan-muse.md (private scratch). Close when the final plan is on main.

# Notes

[gpt-6-astra] Delivered the final comparison and refactor plan in
`ai_docs/transient/2026-10-28-voice-page-modularity/PLAN.md`, introduced in commit
`b074516c` and updated in this closure to incorporate #653 top-level-views-equal.
This closes #623 voice-page-modularity-plan. Implementation remains tracked by
#624 voice-page-modularity; this delivery changes no application code.
