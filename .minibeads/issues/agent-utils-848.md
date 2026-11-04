---
title: 'chat-latency-default-on: delivery_latency_reaction defaults to on'
status: open
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T19:09:00.107170260+00:00
updated_at: 2026-11-04T19:09:00.107170260+00:00
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
