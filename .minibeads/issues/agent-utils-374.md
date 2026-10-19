---
title: 'registered-writable-follows-policy: a channel added in the app follows its provider''s current write policy, and Settings says why a channel is read only'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T16:42:11.061339057+00:00
updated_at: 2026-10-19T16:42:11.061339057+00:00
---

# Description

[opus 5.5 coordinator / claude worker] Owner, 2026-10-19: on a Slack deployment, `registered_channels_writable` was raised from false to true and the server restarted. The channel listed in the configuration file could then be posted to, but a channel added in the app while the policy was false still said it was read only.

Cause: a channel added in the app keeps the `writable` it was given when it was added (the store's added-channels row), and a Slack provider's `registered_channels_writable` is read only when a channel is registered. So changing the policy never reaches channels added before.

Wanted:
- A channel added in the app through a provider that sets a policy for such channels (Slack with `channel_registration` on and its `registered_channels_writable`) follows the policy as it is now: raising it makes the channels added earlier writable, lowering it makes them read only, from the next start.
- A `[[channels]]` entry in the configuration file keeps its own explicit `writable`.
- A channel added where no provider policy applies (an id added directly with or without "Let this app post here", or a bridge's registration answer) keeps what was chosen when it was added.
- Backward compatible: no store change is needed to read old rows.
- Settings says why a channel is read only: the configuration file, the service's policy for channels added in the app, the choice made when it was added, or a service no longer configured.

Tests: a policy raised and lowered over the same stored channels across a restart, with the configured channel unchanged; Settings' sentence for each reason; negative controls.
