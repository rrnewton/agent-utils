---
title: 'vibe-talk-voice-talker-thinker-design: design the voice agent as a hands-free talker driving the app with a server-side thinker'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-22T10:25:00-05:00
updated_at: 2026-11-22T10:25:00-05:00
---

# Description

The owner wants a major new phase: the voice agent becomes a hands-free mode for the whole app (talker), with the server-side Discuss agent as the thinker, driving the channel and reader views and kept aware of what is on screen.

# Design

A design document for discussion: the talker/thinker split (a fast voice model talks; the server-side Discuss agent thinks via the cache-only reader); hands-free app-driving tools (find a message and go there, read it, open or create a targeted summary, follow links, scroll, read a summary with highlighting, interrupt and quick questions); a proactive on-screen context feed (view, clicked message, open summary, reader position) with privacy and size budgets; relation to existing talker/thinker designs; adding the speech-to-speech model to the summarization bake-off for a data-driven split; the path off the old voice stack and its blockers; risks and a phased plan.

# Acceptance Criteria

- The design document is published and linked for the owner; no build until the owner picks.

# Notes

Filed by the coordinator; the owning lane refines this file.
