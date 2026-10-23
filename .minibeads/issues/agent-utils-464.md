---
title: 'slack-tail-shapes: accept the record shapes the Slack tail really writes, and log why one is refused'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- slack
- live
created_at: 2026-10-23T16:34:02.784667734+00:00
updated_at: 2026-10-23T16:34:02.784667734+00:00
---

# Description

[opus 5.5] Found on the first deployment of #452 slack-live-ingest: the first record was refused with HTTP 400 and the server logged no reason. The stream writes container_id as the workspace and the conversation (T…:C…), which the parser compared with slack_message.channel_id and refused. Log every refusal at WARN by field and rule (never a value); accept the workspace-prefixed container; acknowledge well-formed records with nothing to publish (client notices without a sequence, creations without a Slack message, system events, reactions that name no conversation, other kinds) as ignored rather than refusing them; fake-test bot posts, broadcasts, file shares, replies and reactions in the captured shape.

# Acceptance Criteria

The captured record shape and its variants are published or ignored through /api/v1/live/tail, never refused; a refusal leaves one WARN line naming the field and rule and none of the record's text.
