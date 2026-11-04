---
title: 'chat-latency-default-on: delivery_latency_reaction defaults to on'
status: closed
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T19:09:00.107170260+00:00
updated_at: 2026-11-04T22:48:23.000000000+00:00
closed_at: 2026-11-04T22:48:23.000000000+00:00
---

# Description

The owner: make `delivery_latency_reaction` (#802 chat-delivery-latency-emoji) default-on in agentctl. Today it defaults to `false`, and every bridge has to opt in.

# Design

- Change the default to `true`. An explicit `false` in a bridge's saved config still turns it off. Per the userguide, a key left absent follows the agentctl default, so absent bridges switch on when they move to the new agentctl.
- Update the userguide ("`delivery_latency_reaction` (default `true`, ...)"), `chat init` help, and the tests that pin the old default.

# Acceptance Criteria

- Tests: absent key = on; explicit false = off; explicit true = on.
- The userguide and help show the new default; gate passes.

# Notes

Owner reply via the lead (2026-11-04). Assigned to cl-start after #842 wrkviz-muse-backend-rust.

## Resolution

[opus 5.5] cl-start, 2026-11-04.

- `BridgeConfiguration::delivery_latency_reaction` is now `Option<bool>`. `delivery_latency_on()` reads it, and an absent key follows `DEFAULT_DELIVERY_LATENCY_REACTION` (`true`). Every reader goes through that method: the receipt emoji, the refused-delivery path, and `chat status` JSON (which reports the effective value).
- The saved `bridge.json` keeps an absent key absent, so a later default still reaches it. An explicit `false` is written and keeps the reaction off. Earlier builds never wrote `false` (they omitted it), so every bridge that never set the key to `true` switches on with this build. A bridge that wants it off sets `"delivery_latency_reaction": false` explicitly.
- Tests: in `delivery_latency_emoji_maps_each_boundary`, absent is on (and stays absent through a save), explicit `false` is off (and is saved), and explicit `true` is on. The #809 forward-compat test now builds its "every feature off" bridge with the key absent, which keeps the file in its original shape. The test fixtures that assume the reaction is off say `Some(false)`.
- Userguide: "default `true`", plus the absent and explicit-`false` rules. `chat init` help names none of the reaction keys (they are set by editing the saved configuration), so there was no help text to change.
