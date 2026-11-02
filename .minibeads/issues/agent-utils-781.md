---
title: 'chat-no-stale-replies-on-restart: a restarted bridge must not post long-queued replies unasked'
status: open
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T17:39:26.214515020+00:00
updated_at: 2026-11-02T17:39:26.214515020+00:00
---

# Description

[opus 5.5] A consumer team's restore (2026-11-02, their gap 28): after a bridge refresh to a new build, the bridge posted to the owner a reply that had sat in its outbound queue for 4 hours. A restarted bridge must not post queued replies older than a bound (configurable; default e.g. 15 min), or replies to requests already answered by a later reply, without the coordinator's say-so: list them (chat status / health and a prompt to the coordinator) and let it send, edit or drop each one. Fresh replies still go out immediately. Tests: a stale queued reply survives a restart and is held and listed, not posted; a fresh one is posted.
