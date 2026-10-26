---
title: 'page-build-stale-notice: a page running an older bundle than the server serves says so and offers a reload'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
depends_on:
  agent-utils-528: related
  agent-utils-223: related
created_at: 2026-10-26T12:09:57.457885576+00:00
updated_at: 2026-10-26T13:24:27.769661164+00:00
closed_at: 2026-10-26T13:24:27.769660993+00:00
---

# Description

[opus 5.5] While looking into #528 thread-card-markdown-and-cap the owner asked "Maybe I'm running a stale version". The shell is revalidated on every launch (#223 asset-caching), so a restarted app runs the deployed build, but a page left open across a deploy keeps its old code and nothing on it says which build it is. The coordinator (2026-10-26): the server reports the page bundle hash; a page running an older bundle shows a small 'New version — reload' chip; never auto-reload mid-call or mid-typing.

# Acceptance Criteria

The server reports the page bundle's content hash on its reads; a page whose own bundle differs shows a small New version chip that reloads on a press, and never reloads by itself; Settings names the page's build; page tests and a server test.
