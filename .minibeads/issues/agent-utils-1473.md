---
title: 'remove-server-todo-endpoint: remove the server to-do endpoint the page no longer reads'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-14T16:14:54.000000000+00:00
updated_at: 2026-11-14T16:48:09.000000000+00:00
closed_at: 2026-11-14T16:48:09.000000000+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1168 read-toggle-two-way and #1399 toggle-labels-active-style (owner reply 028 dropped the page's to-do view): the page no longer reads `GET /api/v1/channels/{id}/todo`, but the server endpoint is still routed. Its only consumer was the page view removed in f6524cc2; the dismiss and restore acts stay, because the page still uses them for its Done state outside any to-do list.

# Design

- Remove the `GET /api/v1/channels/{channel_id}/todo` route, its handler and response type, and the `ops` to-do view behind it, including its cache consumer kind.
- Keep `POST /dismiss` and `POST /restore` and the dismissed-state reads the timeline, page and agent tools share; only the filtered to-do listing goes.
- Grep first for every other consumer (page, CLI, MCP tools, tests, docs). If a real consumer remains, stop instead of removing.
- Update the route inventory, the endpoint documentation and the tests that read the listing; the remaining Done behaviour is verified through the reads that stay.

# Acceptance Criteria

- `GET /api/v1/channels/{id}/todo` answers 404; no page, CLI or MCP path calls it.
- Dismiss, restore and the shared dismissed-state reads behave as before.
- The focused test for the removed route passes and the vibe-talk validation is green.

# Notes

Owner reply 028 via the host ledger (follow-up to #1168/#1399). Lane mu-trackersync. The page view went away with f6524cc2; this removes the server half on its own gate.

## Resolution

[watermelon-full-preview] Removed with this closing commit. The `GET /api/v1/channels/{id}/todo` route, its handler and response type, the `ops` to-do view and its cache consumer kind are gone; `POST /dismiss`, `POST /restore` and the shared dismissed-state reads stay. The consumer grep found no page fetch (removed in f6524cc2), no CLI caller and no MCP tool; the tests that read the listing now verify the same Done behaviour through the reads that stay, and a focused test pins the removed route at 404.
