---
title: 'voice-channel-scope: a channel selector with All on the voice view that scopes the agent''s default channels'
status: open
priority: 2
issue_type: feature
depends_on:
  agent-utils-624: blocks
created_at: 2026-10-28T19:51:27.668778263+00:00
updated_at: 2026-10-28T19:51:27.672549662+00:00
---

# Description

[opus 5.5] Item 5 of #649 voice-agent-tools, held for #624 voice-page-modularity. Owner 2026-10-28: 'I do want to have a channel selector on the voice view as well with an option for all.' Server side exists since bea8f16b: recent_activity channel_ids (ids, names or a part of a name). Plan: a per-session default scope set by the page (at voice-session and on change) that recent_activity honours when no channel is given and names in its answer; the selector in the voice view's composer row. See ai_docs/transient/2026-10-28-voice-agent-tools/README.md.
