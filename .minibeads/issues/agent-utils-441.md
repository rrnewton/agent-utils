---
title: 'voice-honest-answers: the voice agent claims abilities it lacks and invents reasons for failures'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-22T20:24:12.294133383+00:00
updated_at: 2026-10-22T20:24:12.294133383+00:00
---

# Description

[opus 5.5] Asked how long a build took in a coding agent container, the voice agent said "Checking the build container logs now", called digest_channel twice (both really timed out at the 15 s runtime limit, before the #437 deploy), then said it "tried twice to check the build logs in the offline container" and blamed "the chat connection". It can only read chat messages. Earlier it also sent a chat question to a runtime-provided web search. The prompt and tool descriptions should list its real abilities, forbid claiming to check logs, containers or systems, require answers from message text with the message named, a plain "the messages do not say" when they do not, and a failure report that names the tool that failed.

# Acceptance Criteria

Prompt v12 and tool/initialize text carry these rules; tests pin them; gate green; landed.
