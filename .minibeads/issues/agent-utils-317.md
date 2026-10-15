---
title: 'slack-group-dm-names: name a Slack group DM by its members, not its channel handle'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.392870939+00:00
updated_at: 2026-10-15T06:03:00.392870939+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). A group DM is named from its mpdm-<handle>--<handle> channel name, so in a workspace whose handles are numeric it shows as 'Group: 123456, 234567, 345678' and the owner cannot tell group DMs apart or find one by a person's name. Name group DMs (in the channel browser, the channel picker, headers and speech) from their members' display names, excluding the owner, using conversations.members and the users.info cache vibe-talk already keeps for authors; fall back to the handles only when no name is known. Test against tests/slack_fake with numeric and word handles.
