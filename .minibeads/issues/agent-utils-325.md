---
title: 'provider-diagnostics-ids: Slack checks report under Discord ids'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.421406146+00:00
updated_at: 2026-10-15T08:01:48.114966757+00:00
closed_at: 2026-10-15T08:01:48.114966627+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). On a Slack deployment the diagnostics page reports its Slack checks under the ids discord.token and discord.channel while their titles say Slack. Give each provider's checks provider-specific ids (keeping the old ids as aliases if anything reads them), and show the token owner's display name rather than a numeric handle when the provider gives one.
