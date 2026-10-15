---
title: 'slack-mention-names: draw and speak a Slack mention as the person''s name'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.396811676+00:00
updated_at: 2026-10-15T06:03:00.396811676+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). User mentions are kept in content as <@U...> on purpose, but the page draws the chip as the raw member id ('@U0...') and speech says only 'a mention', so neither the reader nor the voice agent learns who was addressed (7 of 16 messages in one channel). Draw and speak the member's display name (the users.info cache; a bounded lookup for unknown ids), keeping <@U...> in content and in what is posted. Also cover user groups and channel mentions (<!subteam^...>, <#C...|name>). Tests: page rendering, speech text, an unknown id.
