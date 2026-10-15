---
title: 'slack-edited-mark: show that a Slack message was edited'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.403773564+00:00
updated_at: 2026-10-15T07:43:42.872561703+00:00
closed_at: 2026-10-15T07:43:42.872561573+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). Slack history and replies carry an edited object ({user, ts}) on edited messages, but vibe-talk's Slack reader ignores it and the message model has no edited state, so an edited Slack message never shows as edited. Carry an optional edited time through the model and contract (absent means not reported), map Slack's edited.ts, map the equivalent field for other providers that report it, and show a small 'edited' mark on the page (and nothing in speech unless asked). Tests: tests/slack_fake, contract samples, page rendering.
