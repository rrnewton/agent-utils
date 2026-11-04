---
title: 'chat-bound-agent-alternate-urls: the bridge publishes the bound agent''s alternate URLs and vibe-talk offers them'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-04T22:42:10.334805187+00:00
updated_at: 2026-11-04T22:42:10.334805187+00:00
---

# Description

The owner (2026-11-04): "[the agentcloud web app] is a pretty sweet PWA ... I would like a little more metadata through agentctl chat bridge. If we knew that the connected session is an agentcloud session we could have a hamburger icon button to open it in [the web app], a different way to talk to the same agent that is bound to the current slack channel. Maybe a generic metadata concept of the agent bound to a channel having an alternate direct URL?"

# Design

- The bridge publishes bound-agent metadata per channel: harness, host, agent name, and `alternate_urls: [{label, url}]` (for example "Open in the agentcloud web app" for agentcloud sessions; perhaps the Herdr tab or a transcript URL for others). It goes out through its event sink or a small status file.
- vibe-talk shows the entries in the channel's hamburger menu ("Open agent in ...").
- The page part follows the #624 voice-page-modularity linearization rule (new state gets one owner in a module).

# Acceptance Criteria

- Bridge tests for the metadata shape and its update on repoint.
- A page test showing the menu entry for a channel whose bound agent has an alternate URL, and no entry otherwise.

# Notes

P3 owner TODO (2026-11-04 17:38 ET via the lead); no lane yet.
